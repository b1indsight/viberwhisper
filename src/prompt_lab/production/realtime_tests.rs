//! The lab's report contract must retain raw audio-stage text even when integrated preparation
//! applies a saved correction. The peer only implements transport, never recognition.
use super::*;
use std::{
    net::TcpStream,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio_tungstenite::tungstenite::{Message, WebSocket};

fn send(socket: &mut WebSocket<TcpStream>, value: Value) {
    socket.send(Message::text(value.to_string())).unwrap();
}

// Like dictation::test_support, skip upload packets while keeping control-event order strict.
fn event(socket: &mut WebSocket<TcpStream>, expected: &str) -> Value {
    loop {
        let message = socket.read().unwrap();
        let text = message.to_text().unwrap();
        assert!(!text.contains(REFERENCE) && !text.contains("ReferenceOnly"));
        let value: Value = serde_json::from_str(text).unwrap();
        if value["type"] == "input_audio_buffer.append" {
            continue;
        }
        assert_eq!(value["type"], expected);
        return value;
    }
}

/// Serve the supplied recordings on one socket and retain their actual model inputs.
fn realtime_service(
    integrated: bool,
    outputs: Vec<String>,
) -> (String, thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("ws://{}/v1/realtime", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "no production Realtime connection"
                    );
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
        for (index, output) in outputs.into_iter().enumerate() {
            let prefix = format!("prefix-{index}");
            if integrated {
                let declaration = event(&mut socket, "conversation.item.create");
                let mut item = declaration["item"].clone();
                requests.push(declaration);
                item["id"] = json!(prefix);
                item["status"] = json!("completed");
                for kind in ["conversation.item.added", "conversation.item.done"] {
                    send(&mut socket, json!({"type":kind,"item":item}));
                }
            }
            event(&mut socket, "input_audio_buffer.commit");
            let audio = format!("audio-{index}");
            send(
                &mut socket,
                json!({"type":"input_audio_buffer.committed","item_id":audio}),
            );
            let request = event(&mut socket, "response.create");
            let text = if integrated {
                json!({"text":output,"context":"项目","memory_candidates":["维伯耳语"]}).to_string()
            } else {
                output
            };
            let id = format!("response-{index}");
            for response in [
                json!({"type":"response.created","response":{"id":id,"metadata":request["response"]["metadata"]}}),
                json!({"type":"response.output_text.done","response_id":id,"output_index":0,"content_index":0,"text":text}),
                json!({"type":"response.done","response":{"id":id,"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}]}}),
            ] {
                send(&mut socket, response);
            }
            requests.push(request);
            for id in std::iter::once(&audio).chain(integrated.then_some(&prefix)) {
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
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
            "ordinary recordings must reuse the sole production connection"
        );
        requests
    });
    (url, server)
}

#[test]
fn whitespace_realtime_output_saves_incomplete_report_without_losing_raw_evidence() {
    // Valid model whitespace has no deliverable final in main; the lab must still save its
    // failure evidence rather than rejecting the entire run during report validation.
    let raw = " \t\n\u{3000}";
    for integrated in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let store = ready_store(directory.path(), SPEECH);
        let (url, server) = realtime_service(integrated, vec![raw.into()]);
        let mut document = document("http://unused.invalid");
        document.dictation.enabled = true;
        document.dictation.url = url;
        document.post_process.enabled = !integrated;
        let path = directory.path().join("whitespace.json");
        let report = evaluate(&store, request(document, path.clone()))
            .unwrap()
            .report;
        server.join().unwrap();
        let saved = EvaluationReport::read(&path).unwrap();
        assert_eq!(saved.status, RunStatus::Incomplete);
        assert_eq!(saved.failures.len(), 1);
        assert!(saved.samples[0].hypothesis.is_none());
        let execution = saved.samples[0].execution.as_ref().unwrap();
        assert_eq!(execution.raw_text, raw);
        assert!(execution.final_text.is_none());
        assert_eq!(execution.cleanup, Cleanup::Skipped);
        assert!(!execution.confirmation_available && !execution.feedback_applied);
        assert_eq!(execution.before, execution.after);
        assert_eq!(execution.feedback, Feedback::Discard);

        // Missing final is allowed only for empty/whitespace raw; neither a fabricated
        // nonempty final nor a nonempty raw without a final may pass report loading.
        for forged_final in [false, true] {
            let mut forged = report.clone();
            let execution = forged.samples[0].execution.as_mut().unwrap();
            if forged_final {
                execution.final_text = Some("fabricated final".into());
            } else {
                execution.raw_text = "nonempty raw".into();
                execution.recognition_error = Some("incomplete audio input".into());
            }
            forged.evidence_sha256 = forged.evidence_digest().unwrap();
            forged.write(&path).unwrap();
            assert!(
                EvaluationReport::read(&path)
                    .unwrap_err()
                    .to_string()
                    .contains("pass-through final differs from raw text")
            );
        }
    }
}

#[test]
fn ordinary_integrated_evaluation_reuses_production_memory_and_connection_without_source_writes() {
    // Ordinary runs must keep main's live clock and memory across WAVs while confirming only
    // the displayed final in a temporary copy; dataset references remain scoring-only data.
    let directory = tempfile::tempdir().unwrap();
    let store = ready_store(directory.path(), SPEECH);
    add_ready(&store, 2, SPEECH);
    let (url, server) = realtime_service(true, vec![CLEANED.into(); 2]);
    let mut document = document("http://unused.invalid");
    document.dictation.enabled = true;
    document.dictation.url = url;
    document.post_process.enabled = false;
    let wall_started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let seed: MemorySeed = serde_json::from_value(json!({
        "long_term":document.dictation.memory.long_term,
        "observations":[{
            "recording":"personal-history","segment":0,"at":wall_started - 1,
            "context":"项目","text":"先前使用ViberWhisper。",
            "terms":[{"context":"项目","kind":"proper_noun","text":"ViberWhisper"}]
        }],
        "corrections":[{
            "context":"项目","rule":{"from":"维伯耳语","to":"ViberWhisper"},
            "recording":"personal-correction","at":wall_started - 1
        }]
    }))
    .unwrap();
    let source = directory.path().join("personal-memory.json");
    seed.write(&source, wall_started).unwrap();
    let bytes = std::fs::read(&source).unwrap();
    let modified = std::fs::metadata(&source).unwrap().modified().unwrap();
    let path = directory.path().join("ordinary-integrated.json");
    let mut request = request(document, path.clone());
    request.memory_path = Some(source.clone());
    let report = evaluate(&store, request).unwrap().report;
    let requests = server.join().unwrap();
    let wall_finished = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    assert!(report.realtime.is_none());
    assert!(report.failures.is_empty());
    assert_eq!(report.memory_seed.as_ref(), Some(&seed));
    assert!(EvaluationReport::read(&path).is_ok());
    let identity = report.execution.as_ref().unwrap();
    assert_eq!(
        requests[0]["session"]["instructions"],
        identity.audio.prompt.as_deref().unwrap()
    );
    assert_eq!(identity.clock, "production-system-clock-v1");
    assert_eq!(identity.feedback, "confirm-unedited-final-isolated-v1");
    assert!((wall_started..=wall_finished).contains(&(report.created_at_unix_ms / 1000)));
    assert_eq!(report.samples.len(), 2);
    let first = report.samples[0].execution.as_ref().unwrap();
    let second = report.samples[1].execution.as_ref().unwrap();
    assert_eq!(second.before, first.after);
    for (index, sample) in report.samples.iter().enumerate() {
        let execution = sample.execution.as_ref().unwrap();
        assert_eq!(execution.raw_text, CLEANED);
        assert_eq!(execution.final_text.as_deref(), Some("使用ViberWhisper。"));
        assert_eq!(sample.raw_hypothesis.as_deref(), Some(CLEANED));
        assert_eq!(sample.hypothesis, execution.final_text);
        assert!(execution.confirmation_available && execution.feedback_applied);
        assert_eq!(
            execution.feedback,
            Feedback::Delivered {
                text: "使用ViberWhisper。".into()
            }
        );
        assert!(
            !serde_json::to_string(&execution.feedback)
                .unwrap()
                .contains("ReferenceOnly")
        );
        assert_eq!(execution.asr.len(), 1);
        assert_eq!(execution.asr[0].output["text"], CLEANED);
        for at in [
            execution.started_at,
            execution.finished_at,
            execution.asr[0].at.unwrap(),
            execution.feedback_at.unwrap(),
        ] {
            assert!(
                (wall_started..=wall_finished).contains(&at),
                "synthetic timestamp: {at}"
            );
        }
        let after = execution
            .after
            .contexts
            .iter()
            .find(|c| c.id == "项目")
            .unwrap();
        assert_eq!(after.history[0], "使用ViberWhisper。");
        assert!(after.history.contains(&"先前使用ViberWhisper。".into()));
        assert_eq!(after.candidates[0].supporting_recordings, index + 2);
    }

    // Inspect the peer's bytes, not just the report's reconstructed memory snapshots.
    assert_eq!(requests.len(), 5); // One session update and two prefix/response pairs.
    for (index, pair) in requests[1..].as_chunks::<2>().0.iter().enumerate() {
        let prefix: Value =
            serde_json::from_str(pair[0]["item"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            prefix["contexts"][0]["long_term"],
            serde_json::to_value(&seed.long_term).unwrap()
        );
        let input: Value = serde_json::from_str(
            pair[1]["response"]["input"][2]["content"][0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let context = input["contexts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "项目")
            .unwrap();
        assert!(
            context["history"]
                .as_array()
                .unwrap()
                .contains(&json!("先前使用ViberWhisper。"))
        );
        assert_eq!(
            context["corrections"][0],
            json!({"from":"维伯耳语","to":"ViberWhisper"})
        );
        assert_eq!(context["candidates"][0]["supporting_recordings"], index + 1);
        assert_eq!(
            context["history"][0],
            if index == 0 {
                "先前使用ViberWhisper。"
            } else {
                "使用ViberWhisper。"
            }
        );
    }
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(&source).unwrap().modified().unwrap(),
        modified
    );
}

#[test]
fn seeded_integrated_evaluation_preserves_raw_and_prepared_final_and_validates_clocks() {
    let (url, server) = realtime_service(true, vec![CLEANED.into(); 2]);
    let directory = tempfile::tempdir().unwrap();
    let store = ready_store(directory.path(), SPEECH);
    let second = add_ready(&store, 2, SPEECH);
    let first = store.list_samples(None).unwrap()[0].id.clone();
    let cases: RealtimeCases = serde_json::from_value(json!({
        "schema_version":3,"cases":[{"id":"correction","at":100,"seed":{
            "long_term":[{"context":"项目","kind":"proper_noun","text":"ViberWhisper"}]
        },"steps":[
            {"sample_id":first,"feedback":{"delivered":{"text":"使用ViberWhisper。"}},
             "expect":{"audio":{"audio_only":false,"candidates_present":["维伯耳语"]},"final_text":CLEANED}},
            {"sample_id":second,"expect":{"raw_text":CLEANED,"final_text":"使用ViberWhisper。"}}
        ]}]
    })).unwrap();
    let mut document = document("http://unused.invalid");
    document.dictation.enabled = true;
    document.dictation.url = url;
    document.post_process.enabled = false;
    let path = directory.path().join("integrated.json");
    let mut request = request(document, path.clone());
    request.cases = Some(cases);
    let report = evaluate(&store, request).unwrap().report;
    server.join().unwrap();
    assert!(EvaluationReport::read(&path).is_ok());
    assert!(report.realtime.as_ref().unwrap().passed());
    let execution = report.samples[1].execution.as_ref().unwrap();
    assert_eq!(execution.raw_text, CLEANED);
    assert_eq!(execution.final_text.as_deref(), Some("使用ViberWhisper。"));
    assert_eq!(execution.asr[0].output["text"], CLEANED);
    assert!(execution.before.contexts[0].corrections[0].to == "ViberWhisper");
    let identity = report.execution.as_ref().unwrap();
    let trace = &execution.asr[0];
    assert!((execution.started_at..=execution.finished_at).contains(&trace.at.unwrap()));
    for at in [
        None,
        Some(execution.started_at - 1),
        Some(execution.finished_at + 1),
    ] {
        let mut forged = report.clone();
        forged.samples[1].execution.as_mut().unwrap().asr[0].at = at;
        forged.evidence_sha256 = forged.evidence_digest().unwrap();
        forged.write(&path).unwrap();
        assert!(
            EvaluationReport::read(&path)
                .unwrap_err()
                .to_string()
                .contains("timestamp")
        );
    }
    let mut backwards = execution.clone();
    backwards.asr[0].at = Some(execution.finished_at);
    let mut next = trace.clone();
    next.index += 1;
    next.audio_start_sample = next.audio_end_sample;
    next.audio_end_sample = next.audio_start_sample.map(|start| start + 1);
    next.at = Some(execution.started_at);
    backwards.asr.push(next);
    assert!(
        backwards
            .validate(identity)
            .unwrap_err()
            .to_string()
            .contains("nonmonotonic")
    );
    let mut forged = report.clone();
    let result = forged.samples[1].execution.as_mut().unwrap();
    result.final_text = Some(CLEANED.into());
    // Keep derived scenario checks and the evidence hash consistent with the forged final,
    // so rejection must come from production correction replay, not a stale digest/check.
    forged.samples[1].hypothesis = Some(CLEANED.into());
    let checks = forged.realtime.as_ref().unwrap().cases.cases[0].steps[1]
        .expect
        .check_production(forged.samples[1].execution.as_ref().unwrap());
    forged.realtime.as_mut().unwrap().steps[1].checks = checks;
    forged.evidence_sha256 = forged.evidence_digest().unwrap();
    forged.write(&path).unwrap();
    let error = EvaluationReport::read(&path).unwrap_err().to_string();
    assert!(error.contains("local correction"), "{error}");
}
