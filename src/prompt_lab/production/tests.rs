use super::*;
use crate::prompt_lab::{
    CaptureTranscription, DatasetStore, EvaluationReport, EvaluationRequest, ProperNounAnnotation,
    RunStatus, Thresholds, apply_review, evaluate, score_wer,
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::Duration,
};

const RAW: &str = "音频原文";
const CLEANED: &str = "使用维伯耳语。";
const REFERENCE: &str = "ReferenceOnly Codex";
const SPEECH: &[u8] = include_bytes!("../../../tests/fixtures/audio/speech.wav");

struct Service {
    url: String,
    requests: mpsc::Receiver<(String, Vec<u8>)>,
    thread: thread::JoinHandle<()>,
}

/// A bounded local HTTP peer, not a replacement recognizer: PCM still traverses WAV replay,
/// production buffering/orchestration, the real multipart client and real structured cleanup.
fn service(request_count: usize, fail_last_audio: bool) -> Service {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (sent, received) = mpsc::channel();
    let thread = thread::spawn(move || {
        for index in 0..request_count {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "production request did not reach local service"
                        );
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            // macOS can inherit the listener's nonblocking flag on accepted sockets.
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
                if let Some(index) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
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
                .to_string();
            let body = bytes[header_end..header_end + length].to_vec();
            let failed = fail_last_audio && index + 1 == request_count;
            let response = if failed {
                assert_eq!(route, "/stt");
                json!({"error":"private-provider-body"})
            } else if route == "/stt" { json!({"text":RAW}) }
                else {
                    assert_eq!(route, "/cleanup");
                    json!({"choices":[{"message":{"content":json!({"text":CLEANED, "context":"项目", "memory_candidates":["维伯耳语"]}).to_string()}}]})
                }.to_string();
            let status = if failed { "400 Bad Request" } else { "200 OK" };
            write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
            sent.send((route, body)).unwrap();
        }
    });
    Service {
        url,
        requests: received,
        thread,
    }
}

fn document(url: &str) -> ConfigDocument {
    let mut document = ConfigDocument::default();
    document.inference.api.transcription.api_url = format!("{url}/stt");
    document.inference.api.transcription.model = "audio-fixture".into();
    document.post_process.enabled = true;
    document.post_process.preheat_enabled = false;
    document.inference.api.post_process.api_url = Some(format!("{url}/cleanup"));
    document.inference.api.post_process.model = Some("text-fixture".into());
    document.dictation.memory.long_term = vec![crate::dictation::config::LongTerm {
        context: "private".into(),
        kind: crate::dictation::config::TermKind::ProperNoun,
        text: "ConfiguredPersonalSentinel".into(),
    }];
    document
}

fn wav(path: &Path) {
    // A sine wave is audible but correctly rejected by production speech detection. Use the
    // existing real-speech fixture rather than bypassing that gate to exercise both API stages.
    std::fs::write(path, SPEECH).unwrap();
}

fn ready_store(root: &Path, audio: &[u8]) -> DatasetStore {
    let store = DatasetStore::open_or_create(root.join("dataset")).unwrap();
    add_ready(&store, 1, audio);
    store
}

fn add_ready(store: &DatasetStore, at: u64, audio: &[u8]) -> String {
    let reservation = store.reserve_sample(at).unwrap();
    std::fs::write(&reservation.audio_path, audio).unwrap();
    let id = reservation.id.clone();
    store
        .complete_capture(
            reservation,
            CaptureTranscription::success(
                "captured text must never be reused",
                SttSnapshot {
                    endpoint: "http://capture.invalid/stt".into(),
                    model: "capture-only".into(),
                    language: None,
                    prompt: None,
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
    id
}

fn request(document: ConfigDocument, path: PathBuf) -> EvaluationRequest {
    EvaluationRequest {
        document,
        memory_path: None,
        asr_prompt_override: None,
        thresholds: Thresholds {
            max_wer_percent: 1000.0,
            min_llm_score: 0.0,
            min_proper_noun_percent: 0.0,
        },
        output: Some(path),
        compare_to: None,
        cases: None,
    }
}

#[test]
fn plain_http_without_cleanup_ignores_unused_memory_during_report_replay() {
    // A saved vocabulary must not make plain HTTP evaluation fail: this production mode
    // owns no memory, even when the configuration enables it for other recognition modes.
    let directory = tempfile::tempdir().unwrap();
    let store = ready_store(directory.path(), SPEECH);
    let Service {
        url,
        requests,
        thread: server,
    } = service(1, false);
    let mut document = document(&url);
    document.post_process.enabled = false;
    assert!(document.dictation.memory.enabled);
    assert!(!document.dictation.memory.long_term.is_empty());
    let path = directory.path().join("run.json");
    let report = evaluate(&store, request(document, path.clone()))
        .unwrap()
        .report;
    server.join().unwrap();
    let execution = report.samples[0].execution.as_ref().unwrap();
    assert_eq!(report.samples[0].hypothesis.as_deref(), Some(RAW));
    assert_eq!(execution.cleanup, Cleanup::Disabled);
    assert_eq!(execution.before, Snapshot::default());
    assert_eq!(execution.after, Snapshot::default());
    assert!(!execution.confirmation_available && !execution.feedback_applied);
    assert!(EvaluationReport::read(&path).is_ok());
    assert_eq!(
        requests
            .try_iter()
            .map(|(route, _)| route)
            .collect::<Vec<_>>(),
        ["/stt"]
    );
}

#[test]
fn full_production_evaluation_scores_cleaned_final_and_rejects_report_and_review_tampering() {
    let directory = tempfile::tempdir().unwrap();
    let store = ready_store(directory.path(), SPEECH);
    let Service {
        url,
        requests,
        thread: server,
    } = service(2, false);
    let path = directory.path().join("run.json");
    let result = evaluate(&store, request(document(&url), path.clone())).unwrap();
    server.join().unwrap();
    let report = result.report;
    assert_eq!(report.schema_version, 3);
    assert_eq!(report.status, RunStatus::AwaitingAgentReview);
    let sample = &report.samples[0];
    assert_eq!(sample.raw_hypothesis.as_deref(), Some(RAW));
    assert_eq!(sample.hypothesis.as_deref(), Some(CLEANED));
    assert_eq!(
        sample.wer.as_ref().unwrap().alignment,
        score_wer(REFERENCE, CLEANED).alignment
    );
    assert_ne!(
        sample.wer.as_ref().unwrap().alignment,
        score_wer(REFERENCE, RAW).alignment
    );
    let execution = sample.execution.as_ref().unwrap();
    assert_eq!(execution.cleanup, Cleanup::Applied);
    assert_eq!(execution.text.as_ref().unwrap().output["text"], CLEANED);
    // Ordinary lab must keep main's vocabulary and confirm the actual final in its private copy.
    assert_eq!(
        execution.before.contexts[0].long_term[0].text,
        "ConfiguredPersonalSentinel"
    );
    assert!(
        execution
            .after
            .contexts
            .iter()
            .any(|context| context.history.contains(&CLEANED.to_owned()))
    );
    assert!(execution.confirmation_available && execution.feedback_applied);
    assert!(EvaluationReport::read(&path).is_ok());
    for (route, body) in requests.try_iter() {
        let bytes = String::from_utf8_lossy(&body);
        assert!(!bytes.contains(REFERENCE));
        if route == "/cleanup" {
            assert!(bytes.contains("ConfiguredPersonalSentinel"));
            let request: Value = serde_json::from_slice(&body).unwrap();
            let input: Value =
                serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
            assert_eq!(input["text"], RAW);
            assert!(!input["contexts"].as_array().unwrap().is_empty());
        }
    }
    let bytes = serde_json::to_value(&report).unwrap();
    // Each attack changes a different trust boundary, not only its aggregate score.
    for (pointer, value) in [
        ("/samples/0/hypothesis", json!("forged final")),
        ("/execution/cleanup/model", json!("other-text-model")),
        (
            "/samples/0/execution/feedback",
            json!({"delivered":{"text":"reference must not become feedback"}}),
        ),
    ] {
        let mut tampered = bytes.clone();
        *tampered.pointer_mut(pointer).unwrap() = value;
        std::fs::write(&path, serde_json::to_vec(&tampered).unwrap()).unwrap();
        assert!(EvaluationReport::read(&path).is_err(), "{pointer}");
    }
    report.write(&path).unwrap();
    let review_path = directory.path().join("review.json");
    let mut review = json!({ "schema_version":2, "run_id":report.run_id,
        "rubric_version":"semantic-equivalence-v1", "execution_sha256": report.execution.as_ref().unwrap().sha256,
        "evidence_sha256": report.evidence_sha256,
        "samples":[{"sample_id":sample.sample_id, "score":50, "reason":"distinct reference", "differences":[{"category":"meaning", "explanation":"different words"}]}] });
    review["evidence_sha256"] = json!("wrong-evidence");
    std::fs::write(&review_path, serde_json::to_vec(&review).unwrap()).unwrap();
    assert!(apply_review(&path, &review_path).is_err());
    review["evidence_sha256"] = json!(report.evidence_sha256);
    std::fs::write(&review_path, serde_json::to_vec(&review).unwrap()).unwrap();
    assert_eq!(
        apply_review(&path, &review_path).unwrap().status,
        RunStatus::Complete
    );
    assert!(EvaluationReport::read(&path).is_ok());
}

#[test]
fn normal_lab_matches_main_requests_and_learning_except_candidate_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let store = ready_store(directory.path(), SPEECH);
    add_ready(&store, 2, SPEECH);
    let Service {
        url,
        requests,
        thread: server,
    } = service(8, false);
    let mut document = document(&url);
    document.transcription.prompt = Some("main audio instructions".into());
    let now = crate::dictation::system_clock()();
    let source = directory.path().join("personal-memory.json");
    let bytes = serde_json::to_vec(&json!({
        "schema_version":2,
        "observations":[{"recording":"previous","segment":0,"at":now - 10,
            "context":"项目","text":"历史输入","terms":[]}],
        "corrections":[{"context":"项目","rule":{"from":"维伯耳语","to":"ViberWhisper"},
            "recording":"previous-edit","at":now - 10}]
    }))
    .unwrap();
    std::fs::write(&source, &bytes).unwrap();
    let modified = std::fs::metadata(&source).unwrap().modified().unwrap();
    let main_memory = directory.path().join("main-copy.json");
    std::fs::copy(&source, &main_memory).unwrap();
    // Assemble exactly as the desktop host, without native delivery. Its accepted output is the
    // independent oracle for the lab's stateful requests; the peer only returns fixed responses.
    let main = RecognitionPipeline::from_resolved(
        RecognitionConfig::from_config(&document, false).unwrap(),
        PostProcessConfig::from_config(&document).unwrap(),
        document.dictation.memory.clone(),
        document.transcription.language.clone(),
        Some(main_memory),
        crate::dictation::system_clock(),
    )
    .unwrap();
    let audio = directory.path().join("input.wav");
    wav(&audio);
    let mut main_finals = Vec::new();
    for index in 1..=2 {
        let outcome = main
            .recognize_wav(&audio, SessionId(index), &CancellationToken::new())
            .unwrap();
        let transcript = outcome.transcript.unwrap();
        assert_eq!(transcript.text, "使用ViberWhisper。");
        transcript
            .confirmation
            .unwrap()
            .confirm_with_ack(transcript.text.clone())
            .unwrap();
        main_finals.push(transcript.text);
    }
    main.shutdown();
    let path = directory.path().join("run.json");
    let mut request = request(document, path.clone());
    request.memory_path = Some(source.clone());
    request.asr_prompt_override = Some("candidate audio instructions".into());
    let report = evaluate(&store, request).unwrap().report;
    server.join().unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(&source).unwrap().modified().unwrap(),
        modified
    );
    assert!(EvaluationReport::read(&path).is_ok());
    let finals = report
        .samples
        .iter()
        .map(|sample| sample.hypothesis.clone().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(finals, main_finals);
    let first = report.samples[0].execution.as_ref().unwrap();
    let second = report.samples[1].execution.as_ref().unwrap();
    assert!(first.started_at >= now && first.feedback_at.is_some());
    assert_eq!(first.after, second.before);
    assert!(
        second
            .before
            .contexts
            .iter()
            .any(|context| context.history.contains(&"使用ViberWhisper。".into()))
    );
    let requests = requests.try_iter().collect::<Vec<_>>();
    let cleanup = requests
        .iter()
        .filter(|(route, _)| route == "/cleanup")
        .map(|(_, body)| serde_json::from_slice::<Value>(body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(cleanup.len(), 4);
    assert_eq!(cleanup[..2], cleanup[2..]);
    let audio = requests
        .iter()
        .filter(|(route, _)| route == "/stt")
        .map(|(_, body)| body)
        .collect::<Vec<_>>();
    assert_eq!(audio.len(), 4);
    let source_audio = hound::WavReader::new(std::io::Cursor::new(SPEECH)).unwrap();
    let source_spec = source_audio.spec();
    let source_samples = source_audio.len() as usize;
    let mut sent_pcm = Vec::new();
    for (index, body) in audio.iter().enumerate() {
        let text = String::from_utf8_lossy(body);
        assert!(text.contains(if index < 2 {
            "main audio instructions"
        } else {
            "candidate audio instructions"
        }));
        assert!(!text.contains(REFERENCE));
        let start = body.windows(4).position(|bytes| bytes == b"RIFF").unwrap();
        let length =
            u32::from_le_bytes(body[start + 4..start + 8].try_into().unwrap()) as usize + 8;
        // Production adjusts loudness. Compare its actual main/lab uploads, not raw fixture PCM.
        let sent_audio =
            hound::WavReader::new(std::io::Cursor::new(&body[start..start + length])).unwrap();
        assert_eq!(sent_audio.spec(), source_spec);
        let pcm = sent_audio
            .into_samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(pcm.len(), source_samples);
        sent_pcm.push(pcm);
    }
    assert_eq!(sent_pcm[..2], sent_pcm[2..]);
    assert!(!serde_json::to_string(&cleanup).unwrap().contains(REFERENCE));
    let mut forged = report.clone();
    let sample = &mut forged.samples[0];
    let execution = sample.execution.as_mut().unwrap();
    execution.final_text = Some(CLEANED.into());
    execution.text.as_mut().unwrap().prepared_text = CLEANED.into();
    execution.feedback = Feedback::Delivered {
        text: CLEANED.into(),
    };
    sample.hypothesis = Some(CLEANED.into());
    forged.evidence_sha256 = forged.evidence_digest().unwrap();
    forged.write(&path).unwrap();
    assert!(
        EvaluationReport::read(&path)
            .unwrap_err()
            .to_string()
            .contains("local correction")
    );
}

#[test]
fn production_partial_keeps_successful_text_but_remains_incomplete_and_skips_cleanup() {
    // The pause creates two production HTTP segments. The second fails after the first succeeds;
    // retaining that first text must not accidentally turn this into a reviewable successful run.
    let mut reader = hound::WavReader::new(std::io::Cursor::new(SPEECH)).unwrap();
    let spec = reader.spec();
    let samples = reader
        .samples::<i16>()
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
    let store = ready_store(directory.path(), &audio.into_inner());
    let Service {
        url,
        requests,
        thread: server,
    } = service(2, true);
    let path = directory.path().join("partial.json");
    let result = evaluate(&store, request(document(&url), path.clone())).unwrap();
    server.join().unwrap();
    let report = result.report;
    assert_eq!(report.status, RunStatus::Incomplete);
    assert_eq!(report.samples[0].raw_hypothesis.as_deref(), Some(RAW));
    assert_eq!(report.samples[0].hypothesis.as_deref(), Some(RAW));
    assert!(report.samples[0].wer.is_some());
    let execution = report.samples[0].execution.as_ref().unwrap();
    assert_eq!(execution.cleanup, Cleanup::Skipped);
    assert!(execution.recognition_error.is_some() && execution.warning.is_some());
    assert!(!execution.confirmation_available && !execution.feedback_applied);
    assert!(execution.text.is_none() && execution.after == execution.before);
    assert_eq!(
        requests
            .try_iter()
            .map(|(route, _)| route)
            .collect::<Vec<_>>(),
        ["/stt", "/stt"]
    );
    assert!(EvaluationReport::read(&path).is_ok());
    assert!(
        !std::fs::read_to_string(&path)
            .unwrap()
            .contains("private-provider-body")
    );
    let review = directory.path().join("missing-review.json");
    assert!(
        apply_review(&path, &review)
            .unwrap_err()
            .to_string()
            .contains("awaiting_agent_review")
    );
    let mut next = request(document(&url), directory.path().join("next.json"));
    next.compare_to = Some(report);
    assert!(
        evaluate(&store, next)
            .unwrap_err()
            .to_string()
            .contains("not compatible")
    );
}

#[test]
fn explicit_delivery_alone_learns_and_next_production_result_uses_local_correction() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("audio.wav");
    wav(&path);
    let Service {
        url,
        requests,
        thread: server,
    } = service(8, false);
    let document = document(&url);
    let identity = Identity::from_config(&document, None, None, &MemorySeed::default()).unwrap();
    let runner = Runner::new(&document, None, &MemorySeed::default(), 100).unwrap();
    let feedback = [
        Feedback::Discard,
        Feedback::DeliveryFailed,
        Feedback::Delivered {
            text: "使用ViberWhisper。".into(),
        },
        Feedback::Discard,
    ];
    let mut executions = Vec::new();
    for (index, feedback) in feedback.iter().enumerate() {
        let execution = runner
            .recognize(&path, SessionId(index as u64 + 1), 0, Some(feedback))
            .unwrap();
        if index < 2 {
            assert!(execution.after.contexts.is_empty());
        } else {
            assert_eq!(
                execution.after.contexts[0].corrections[0].to,
                "ViberWhisper"
            );
        }
        assert_eq!(
            execution.final_text.as_deref(),
            Some(if index == 3 {
                "使用ViberWhisper。"
            } else {
                CLEANED
            })
        );
        executions.push(execution);
    }
    server.join().unwrap();
    assert_eq!(requests.try_iter().count(), 8);
    validate_replay(&identity, &MemorySeed::default(), &executions).unwrap();
    let mut tampered = executions.clone();
    tampered[2].feedback = Feedback::Discard;
    tampered[2].feedback_applied = false;
    assert!(validate_replay(&identity, &MemorySeed::default(), &tampered).is_err());
    let mut tampered = executions.clone();
    tampered[3].final_text = Some(CLEANED.into());
    tampered[3].text.as_mut().unwrap().prepared_text = CLEANED.into();
    assert!(validate_replay(&identity, &MemorySeed::default(), &tampered).is_err());
}

#[test]
fn comparison_allows_audio_candidate_but_rejects_cleanup_mode_and_policy_changes() {
    let document = document("http://127.0.0.1:1");
    let baseline = Identity::from_config(&document, None, None, &MemorySeed::default()).unwrap();
    let candidate = Identity::from_config(
        &document,
        Some("candidate ASR"),
        None,
        &MemorySeed::default(),
    )
    .unwrap();
    let changes = candidate.compare(&baseline).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].field, "audio.prompt");
    let changed_seed = MemorySeed {
        long_term: document.dictation.memory.long_term.clone(),
        ..Default::default()
    };
    assert!(
        Identity::from_config(&document, Some("candidate ASR"), None, &changed_seed)
            .unwrap()
            .compare(&baseline)
            .unwrap_err()
            .to_string()
            .contains("same initial production memory")
    );
    let mut different = document.clone();
    different.post_process.prompt = Some("other cleanup".into());
    assert!(
        Identity::from_config(&different, None, None, &MemorySeed::default())
            .unwrap()
            .compare(&baseline)
            .is_err()
    );
    different = document.clone();
    different.dictation.enabled = true;
    assert!(
        Identity::from_config(&different, None, None, &MemorySeed::default())
            .unwrap()
            .compare(&baseline)
            .is_err()
    );
    let mut forged = baseline.clone();
    forged.policy = "old-fixed-blocks".into();
    assert!(candidate.compare(&forged).is_err());
    different.dictation.prompt = Some("saved incompatible integrated rules".into());
    assert!(
        Identity::from_config(&different, Some("candidate"), None, &MemorySeed::default())
            .unwrap_err()
            .to_string()
            .contains("conflicts")
    );
}

#[path = "realtime_tests.rs"]
mod realtime_tests;
