use super::*;
use crate::prompt_lab::{
    CaptureTranscription, DatasetStore, EvaluationReport, EvaluationRequest, ProperNounAnnotation,
    RunStatus, Thresholds, evaluate_production,
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const SPEECH: &[u8] = include_bytes!("../../../tests/fixtures/audio/speech.wav");
const RAW: &str = "音频原文";
const CLEANED: &str = "使用维伯耳语。";
const FINAL: &str = "使用ViberWhisper。";
const REFERENCE: &str = "ReferenceOnly Codex";

struct Service {
    url: String,
    requests: mpsc::Receiver<(String, Vec<u8>)>,
    server: thread::JoinHandle<()>,
}

/// Bounded transport peer: real PCM, production clients, parsing and finalization run normally.
fn service(count: usize, fail_last: bool) -> Service {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (sent, received) = mpsc::channel();
    let server = thread::spawn(move || {
        for index in 0..count {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing production request");
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            let header_end = loop {
                let length = stream.read(&mut buffer).unwrap();
                assert_ne!(length, 0);
                bytes.extend_from_slice(&buffer[..length]);
                if let Some(index) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                    break index + 4;
                }
                assert!(bytes.len() <= 16 * 1024);
            };
            let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            assert!(length <= 256 * 1024);
            while bytes.len() < header_end + length {
                let length = stream.read(&mut buffer).unwrap();
                assert_ne!(length, 0);
                bytes.extend_from_slice(&buffer[..length]);
            }
            let route = headers
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_owned();
            let failed = fail_last && index + 1 == count;
            let response = if failed {
                assert_eq!(route, "/stt");
                json!({"error":"private-provider-body"})
            } else if route == "/stt" {
                json!({"text":RAW})
            } else {
                assert_eq!(route, "/cleanup");
                json!({"choices":[{"message":{"content":json!({"text":CLEANED,"context":"项目","memory_candidates":["维伯耳语"]}).to_string()}}]})
            }.to_string();
            let status = if failed { "400 Bad Request" } else { "200 OK" };
            write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
            sent.send((route, bytes[header_end..header_end + length].to_vec()))
                .unwrap();
        }
    });
    Service {
        url,
        requests: received,
        server,
    }
}

fn document(url: &str) -> ConfigDocument {
    let mut document = ConfigDocument::default();
    document.inference.api.transcription.api_url = format!("{url}/stt");
    document.inference.api.transcription.model = "audio-fixture".into();
    document.transcription.prompt = Some("main audio instructions".into());
    document.transcription.temperature = 0.25;
    document.post_process.enabled = true;
    document.post_process.preheat_enabled = false;
    document.inference.api.post_process.api_url = Some(format!("{url}/cleanup"));
    document.inference.api.post_process.model = Some("text-fixture".into());
    document.dictation.memory.long_term = vec![crate::dictation::config::LongTerm {
        context: "项目".into(),
        kind: crate::dictation::config::TermKind::ProperNoun,
        text: "ConfiguredPersonalSentinel".into(),
    }];
    document
}

fn seed(path: &Path) {
    let at = super::super::regression::unix_time_ms().unwrap() / 1000;
    std::fs::write(
        path,
        serde_json::to_vec(&json!({
            "schema_version":2,
            "observations":[{"recording":"previous","segment":0,"at":at - 10,
                "context":"项目","text":"先前使用ViberWhisper。","terms":[]}],
            "corrections":[{"context":"项目","rule":{"from":"维伯耳语","to":"ViberWhisper"},
                "recording":"previous-edit","at":at - 10}]
        }))
        .unwrap(),
    )
    .unwrap();
}

fn dataset(root: &Path, count: u64, audio: &[u8]) -> DatasetStore {
    let store = DatasetStore::open_or_create(root.join("dataset")).unwrap();
    for at in 1..=count {
        let reservation = store.reserve_sample(at).unwrap();
        std::fs::write(&reservation.audio_path, audio).unwrap();
        let id = reservation.id.clone();
        store
            .complete_capture(
                reservation,
                CaptureTranscription::success(
                    "captured text must not be reused",
                    SttSnapshot {
                        endpoint: "http://capture.invalid/stt".into(),
                        model: "capture-only".into(),
                        language: None,
                        prompt: None,
                        short_prompt: None,
                        temperature: 0.0,
                        input_layout: None,
                    },
                ),
            )
            .unwrap();
        store
            .correct_sample(
                &id,
                REFERENCE,
                vec![ProperNounAnnotation {
                    canonical: "Codex".into(),
                    accepted: vec![],
                    case_sensitive: true,
                    expected_occurrences: 1,
                }],
            )
            .unwrap();
    }
    store
}

fn request(runner: &Runner, output: PathBuf) -> EvaluationRequest {
    EvaluationRequest {
        stt: runner.stt.clone(),
        thresholds: Thresholds {
            max_wer_percent: 1000.0,
            min_llm_score: 0.0,
            min_proper_noun_percent: 0.0,
        },
        output: Some(output),
        compare_to: None,
        realtime: None,
    }
}

#[test]
fn ordinary_metadata_records_short_policy_but_literal_overrides_and_http_do_not() {
    // Metadata must describe the actual production candidate, not just its normal branch;
    // explicit prompt-file/no-prompt candidates must never inherit default short examples.
    let mut document = ConfigDocument::default();
    document.post_process.enabled = false;
    document.post_process.preheat_enabled = false;
    document.dictation.enabled = true;
    let runner = Runner::new(&document, None, None).unwrap();
    assert_eq!(runner.stt.prompt, Some(document.dictation.prompt()));
    assert_eq!(runner.stt.short_prompt, document.dictation.short_prompt());
    assert!(runner.stt.short_prompt.is_some());
    assert_eq!(
        runner.identity.policy,
        "desktop-recognition-reference-text-v1"
    );
    drop(runner);
    for prompt in ["candidate", ""] {
        let runner = Runner::new(&document, Some(prompt.into()), None).unwrap();
        assert_eq!(runner.stt.prompt.as_deref(), Some(prompt));
        assert!(runner.stt.short_prompt.is_none());
    }
    document.dictation.prompt = Some("saved literal".into());
    let runner = Runner::new(&document, None, None).unwrap();
    assert_eq!(runner.stt.prompt.as_deref(), Some("saved literal"));
    assert!(runner.stt.short_prompt.is_none());
    drop(runner);
    document.dictation.enabled = false;
    let runner = Runner::new(&document, None, None).unwrap();
    assert!(runner.stt.short_prompt.is_none());
}

#[test]
fn ordinary_http_matches_desktop_pcm_cleanup_and_learning_except_audio_prompt() {
    // Distinct raw, corrected final and reference detect a skipped cleanup/correction stage,
    // lost feedback, or a lab that learns/scores the reference instead of main's final.
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("personal-memory.json");
    seed(&source);
    let bytes = std::fs::read(&source).unwrap();
    let modified = std::fs::metadata(&source).unwrap().modified().unwrap();
    let audio = directory.path().join("input.wav");
    std::fs::write(&audio, SPEECH).unwrap();
    let Service {
        url,
        requests,
        server,
    } = service(8, false);
    let document = document(&url);
    let main_memory = directory.path().join("main-memory.json");
    std::fs::copy(&source, &main_memory).unwrap();
    let config = ListenerConfig::from_config(&document).unwrap();
    let post = PostProcessor::new_desktop(
        config.post_process,
        config.memory,
        Some(main_memory.clone()),
        config.language,
    )
    .unwrap();
    let main = config
        .recording
        .recognition
        .build_with_memory_path(Some(main_memory))
        .unwrap();
    let mut expected = Vec::new();
    for id in 1..=2 {
        let id = SessionId(id);
        main.start_session(id).unwrap();
        crate::audio::replay_wav(
            &audio,
            id,
            false,
            &tokio_util::sync::CancellationToken::new(),
            |chunk| main.on_chunk_ready(id, chunk),
        )
        .unwrap();
        let result = finish_transcription(main.finish_for_delivery(id), &post).unwrap();
        assert_eq!(result.text, FINAL);
        result
            .confirmation
            .unwrap()
            .confirm(result.text.clone())
            .unwrap();
        expected.push(Some(result.text));
    }
    main.shutdown();
    post.shutdown();
    let store = dataset(directory.path(), 2, SPEECH);
    let runner = Runner::new(
        &document,
        Some("candidate audio instructions".into()),
        Some(source.clone()),
    )
    .unwrap();
    let output = directory.path().join("run.json");
    let report = evaluate_production(&store, &runner, request(&runner, output.clone()))
        .unwrap()
        .report;
    drop(runner);
    server.join().unwrap();
    assert_eq!(
        report
            .samples
            .iter()
            .map(|s| s.hypothesis.clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(report.status, RunStatus::AwaitingAgentReview);
    assert_eq!(
        report.samples[0].wer.as_ref().unwrap().wer_percent,
        crate::prompt_lab::score_wer(REFERENCE, FINAL).wer_percent
    );
    assert!(EvaluationReport::read(&output).is_ok());
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(&source).unwrap().modified().unwrap(),
        modified
    );
    let requests = requests.try_iter().collect::<Vec<_>>();
    let cleanup = requests
        .iter()
        .filter(|(route, _)| route == "/cleanup")
        .map(|(_, body)| serde_json::from_slice::<Value>(body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(cleanup[..2], cleanup[2..]);
    let first: Value =
        serde_json::from_str(cleanup[2]["messages"][1]["content"].as_str().unwrap()).unwrap();
    let second: Value =
        serde_json::from_str(cleanup[3]["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(first["text"], RAW);
    assert!(first.to_string().contains("ConfiguredPersonalSentinel"));
    assert!(first.to_string().contains("先前使用ViberWhisper。"));
    assert!(
        !first["contexts"][0]["history"]
            .as_array()
            .unwrap()
            .contains(&json!(FINAL))
    );
    assert!(
        second["contexts"][0]["history"]
            .as_array()
            .unwrap()
            .contains(&json!(FINAL))
    );
    assert!(!serde_json::to_string(&cleanup).unwrap().contains(REFERENCE));
    let mut pcm = Vec::new();
    for (index, (_, body)) in requests
        .iter()
        .filter(|(route, _)| route == "/stt")
        .enumerate()
    {
        let text = String::from_utf8_lossy(body);
        assert!(text.contains(if index < 2 {
            "main audio instructions"
        } else {
            "candidate audio instructions"
        }));
        assert!(text.contains("name=\"temperature\"\r\n\r\n0.25"));
        assert!(!text.contains(REFERENCE));
        let start = body.windows(4).position(|b| b == b"RIFF").unwrap();
        let length =
            u32::from_le_bytes(body[start + 4..start + 8].try_into().unwrap()) as usize + 8;
        pcm.push(
            hound::WavReader::new(std::io::Cursor::new(&body[start..start + length]))
                .unwrap()
                .into_samples::<i16>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
        );
    }
    assert_eq!(pcm.len(), 4);
    assert_eq!(pcm[..2], pcm[2..]);

    // Baselines must not silently compare old STT-only execution, different cleanup rules
    // or different initial memory with this run, even when the audio metadata is unchanged.
    let review = directory.path().join("review.json");
    std::fs::write(
        &review,
        serde_json::to_vec(&json!({
            "schema_version":1,"run_id":report.run_id,"rubric_version":"semantic-equivalence-v1",
            "samples":report.samples.iter().map(|sample| json!({
                "sample_id":sample.sample_id,"score":50,"reason":"distinct reference",
                "differences":[{"category":"meaning","explanation":"different words"}]
            })).collect::<Vec<_>>()
        }))
        .unwrap(),
    )
    .unwrap();
    let complete = crate::prompt_lab::apply_review(&output, &review).unwrap();
    for variant in 0..5 {
        let runner = Runner::new(&document, None, Some(source.clone())).unwrap();
        let mut prior = complete.clone();
        match variant {
            0 => {
                prior.schema_version = 2;
                prior.execution = None;
            }
            1 => prior.execution.as_mut().unwrap().config_sha256 = "1".repeat(64),
            2 => prior.execution.as_mut().unwrap().memory_sha256 = "2".repeat(64),
            3 => prior.execution.as_mut().unwrap().policy = "desktop-recognition-v1".into(),
            _ => {
                prior.execution.as_mut().unwrap().policy =
                    "desktop-recognition-reference-text-v1".into()
            }
        }
        let mut next = request(
            &runner,
            directory.path().join(format!("next-{variant}.json")),
        );
        next.compare_to = Some(prior);
        assert!(
            evaluate_production(&store, &runner, next)
                .unwrap_err()
                .to_string()
                .contains("same execution policy")
        );
    }
}

#[test]
fn partial_audio_retains_main_final_but_cannot_be_reviewed_or_learned() {
    // A real pause makes two production chunks; failure of the second must retain the first
    // as incomplete text and skip cleanup, rather than accidentally reporting a successful run.
    let reader = hound::WavReader::new(std::io::Cursor::new(SPEECH)).unwrap();
    let spec = reader.spec();
    let samples = reader
        .into_samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut audio = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut audio, spec).unwrap();
        for sample in samples
            .iter()
            .copied()
            .chain(std::iter::repeat_n(0, spec.sample_rate as usize * 2))
            .chain(samples.iter().copied())
        {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }
    let directory = tempfile::tempdir().unwrap();
    let store = dataset(directory.path(), 1, &audio.into_inner());
    let Service {
        url,
        requests,
        server,
    } = service(2, true);
    let runner = Runner::new(&document(&url), None, None).unwrap();
    let memory = runner._directory.path().join("dictation-memory.json");
    let initial = std::fs::read(&memory).unwrap();
    let path = directory.path().join("partial.json");
    let report = evaluate_production(&store, &runner, request(&runner, path.clone()))
        .unwrap()
        .report;
    server.join().unwrap();
    assert_eq!(report.status, RunStatus::Incomplete);
    assert_eq!(report.samples[0].hypothesis.as_deref(), Some(RAW));
    assert!(report.samples[0].error.is_some());
    assert_eq!(
        requests
            .try_iter()
            .map(|(route, _)| route)
            .collect::<Vec<_>>(),
        ["/stt", "/stt"]
    );
    assert_eq!(std::fs::read(&memory).unwrap(), initial);
    assert!(EvaluationReport::read(&path).is_ok());
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("private-provider-body")
    );
    assert!(crate::prompt_lab::apply_review(&path, &directory.path().join("absent.json")).is_err());
}

#[test]
fn memory_copy_preserves_hidden_state_and_uses_production_recovery() {
    // A visible Snapshot is truncated; copy the actual canonical store so hidden contexts and
    // correction rules are not lost. Disabled/corrupt input follows main's ephemeral recovery.
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.json");
    seed(&source);
    let bytes = std::fs::read(&source).unwrap();
    let mut stored: Value = serde_json::from_slice(&bytes).unwrap();
    let original = stored["observations"][0].clone();
    for i in 0..6 {
        let mut observation = original.clone();
        observation["recording"] = json!(format!("hidden-{i}"));
        observation["context"] = json!(format!("hidden-{i}"));
        stored["observations"]
            .as_array_mut()
            .unwrap()
            .push(observation);
    }
    std::fs::write(&source, serde_json::to_vec(&stored).unwrap()).unwrap();
    let destination = directory.path().join("copy.json");
    let settings = document("http://unused.invalid").dictation.memory;
    let at = super::super::regression::unix_time_ms().unwrap() / 1000;
    crate::dictation::copy_memory(
        Some(source.clone()),
        destination.clone(),
        settings.clone(),
        at,
    )
    .unwrap();
    let copied: Value = serde_json::from_slice(&std::fs::read(&destination).unwrap()).unwrap();
    assert_eq!(copied, stored);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&source).unwrap()).unwrap(),
        stored
    );
    for disabled in [false, true] {
        std::fs::write(&source, "broken private memory").unwrap();
        let mut settings = settings.clone();
        settings.enabled = !disabled;
        crate::dictation::copy_memory(Some(source.clone()), destination.clone(), settings, at)
            .unwrap();
        let copied: Value = serde_json::from_slice(&std::fs::read(&destination).unwrap()).unwrap();
        assert_eq!(copied["observations"], json!([]));
        assert_eq!(copied["corrections"], json!([]));
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            "broken private memory"
        );
    }
}

#[test]
fn ordinary_realtime_keeps_main_protocol_memory_and_one_connection() {
    // Realtime still owns integrated JSON/learning, even if cleanup is enabled in config.
    // An invalid unused cleanup URL detects accidental two-stage assembly/startup rejection.
    use std::net::TcpStream;
    use tokio_tungstenite::tungstenite::{Message, WebSocket};
    fn send(socket: &mut WebSocket<TcpStream>, value: Value) {
        socket.send(Message::text(value.to_string())).unwrap();
    }
    fn event(socket: &mut WebSocket<TcpStream>, expected: &str) -> Value {
        loop {
            let message = socket.read().unwrap();
            let text = message.to_text().unwrap();
            assert!(!text.contains(REFERENCE));
            let value: Value = serde_json::from_str(text).unwrap();
            if value["type"] == "input_audio_buffer.append" {
                continue;
            }
            assert_eq!(value["type"], expected);
            return value;
        }
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("ws://{}/v1/realtime", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("{error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut socket = tokio_tungstenite::tungstenite::accept(stream).unwrap();
        send(&mut socket, json!({"type":"session.created"}));
        let mut requests = vec![event(&mut socket, "session.update")];
        send(&mut socket, json!({"type":"session.updated"}));
        for index in 0..3 {
            let declaration = event(&mut socket, "conversation.item.create");
            let prefix = format!("prefix-{index}");
            let mut item = declaration["item"].clone();
            requests.push(declaration);
            item["id"] = json!(prefix);
            item["status"] = json!("completed");
            for kind in ["conversation.item.added", "conversation.item.done"] {
                send(&mut socket, json!({"type":kind,"item":item}));
            }
            event(&mut socket, "input_audio_buffer.commit");
            let audio = format!("audio-{index}");
            send(
                &mut socket,
                json!({"type":"input_audio_buffer.committed","item_id":audio}),
            );
            let request = event(&mut socket, "response.create");
            let text = json!({"text":if index == 2 { " \t\n" } else { CLEANED },
                "context":"项目","memory_candidates":["维伯耳语"]})
            .to_string();
            let id = format!("response-{index}");
            for response in [
                json!({"type":"response.created","response":{"id":id,"metadata":request["response"]["metadata"]}}),
                json!({"type":"response.output_text.done","response_id":id,"output_index":0,"content_index":0,"text":text}),
                json!({"type":"response.done","response":{"id":id,"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}]}}),
            ] {
                send(&mut socket, response);
            }
            requests.push(request);
            for id in [&audio, &prefix] {
                assert_eq!(
                    event(&mut socket, "conversation.item.delete")["item_id"],
                    *id
                );
                send(
                    &mut socket,
                    json!({"type":"conversation.item.deleted","item_id":id}),
                );
            }
        }
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
        requests
    });
    let directory = tempfile::tempdir().unwrap();
    let store = dataset(directory.path(), 3, SPEECH);
    let source = directory.path().join("source.json");
    seed(&source);
    let bytes = std::fs::read(&source).unwrap();
    let modified = std::fs::metadata(&source).unwrap().modified().unwrap();
    let mut document = document("http://unused.invalid");
    document.dictation.enabled = true;
    document.dictation.url = url;
    document.dictation.prompt = Some("saved integrated instructions".into());
    document.inference.api.post_process.api_url = Some("unused invalid URL".into());
    let runner = Runner::new(
        &document,
        Some("candidate integrated instructions".into()),
        Some(source.clone()),
    )
    .unwrap();
    let path = directory.path().join("realtime.json");
    let report = evaluate_production(&store, &runner, request(&runner, path.clone()))
        .unwrap()
        .report;
    drop(runner);
    let requests = server.join().unwrap();
    assert_eq!(requests[0]["session"]["instructions"], "");
    assert_eq!(report.status, RunStatus::Incomplete);
    assert_eq!(report.failures.len(), 1);
    assert!(report.samples[2].hypothesis.is_none());
    for sample in &report.samples[..2] {
        assert_eq!(sample.hypothesis.as_deref(), Some(FINAL));
    }
    for (index, pair) in requests[1..].as_chunks::<2>().0.iter().enumerate() {
        assert_eq!(
            pair[1]["response"]["instructions"],
            "candidate integrated instructions"
        );
        let prefix = pair[0]["item"]["content"][0]["text"].as_str().unwrap();
        assert!(prefix.contains("ConfiguredPersonalSentinel"));
        assert!(serde_json::from_str::<Value>(prefix).is_err());
        let reference = pair[1]["response"]["input"][2]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(serde_json::from_str::<Value>(reference).is_err());
        assert!(reference.contains("先前使用ViberWhisper。"));
        assert!(reference.contains("维伯耳语"));
        assert!(reference.contains(&format!(
            "1. {}",
            if index == 0 {
                "先前使用ViberWhisper。"
            } else {
                FINAL
            }
        )));
    }
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(&source).unwrap().modified().unwrap(),
        modified
    );
    assert!(EvaluationReport::read(&path).is_ok());
}
