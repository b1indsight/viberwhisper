//! Exercise the real CLI without personal settings, credentials, or remote services.
// Windows Known Folders ignore these environment overrides. Do not read or write
// the real profile just to test stdout; the API regressions remain platform-neutral.
#![cfg(not(target_os = "windows"))]

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

struct Sandbox {
    directory: TempDir,
}

impl Sandbox {
    fn new(endpoint: &str) -> Self {
        let sandbox = Self {
            directory: tempfile::tempdir().unwrap(),
        };
        // Discover without loading anything, and fail closed if profile isolation failed.
        let output = sandbox.command().args(["config", "path"]).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let path = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim_end());
        assert!(
            path.starts_with(sandbox.directory.path()),
            "refusing to access configuration outside the sandbox: {}",
            path.display()
        );

        let document = serde_json::json!({
            "schema_version": 3,
            "input": {"hold_hotkey": "F8", "toggle_hotkey": "F9"},
            "audio": {"input_device": null, "mic_gain": 1.0},
            "transcription": {"language": "en", "prompt": null, "temperature": 0.0},
            "post_process": {
                "enabled": false,
                "preheat_enabled": false,
                "prompt": null,
                "temperature": 0.0
            },
            "inference": {"api": {
                "transcription": {"api_url": endpoint, "model": "cli-stub-model"},
                "post_process": {}
            }},
            "dictation": {"enabled": false, "memory": {"enabled": false}}
        });
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&document).unwrap()).unwrap();
        sandbox
    }

    fn command(&self) -> Command {
        let root = self.directory.path();
        let mut command = Command::new(env!("CARGO_BIN_EXE_viberwhisper"));
        command
            .env_clear()
            .current_dir(root)
            .stdin(Stdio::null())
            .env("HOME", root)
            .env("XDG_CONFIG_HOME", root.join(".config"))
            .env("XDG_DATA_HOME", root.join(".local/share"))
            .env("XDG_CACHE_HOME", root.join(".cache"))
            .env("TMPDIR", root)
            .env("TEMP", root)
            .env("TMP", root)
            .env("NO_PROXY", "*")
            .env("TRANSCRIPTION_API_KEY", "cli-stub-token");
        // Retain only executable/library lookup settings, never provider or logging settings.
        for name in [
            "PATH",
            "DYLD_LIBRARY_PATH",
            "DYLD_FALLBACK_LIBRARY_PATH",
            "LD_LIBRARY_PATH",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
    }
}

fn convert_speech(status: &'static str, body: &'static str) -> Output {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!(
        "http://{}/v1/audio/transcriptions",
        listener.local_addr().unwrap()
    );
    let sandbox = Sandbox::new(&endpoint);
    let input = sandbox.directory.path().join("speech.wav");
    fs::write(&input, include_bytes!("fixtures/audio/speech.wav")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "CLI never contacted the stub");
                    std::thread::park_timeout(Duration::from_millis(1));
                }
                Err(error) => panic!("failed to accept CLI request: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        {
            let mut request = BufReader::new(&mut stream);
            let mut length = None;
            loop {
                let mut line = String::new();
                assert_ne!(request.read_line(&mut line).unwrap(), 0);
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = Some(value.trim().parse::<u64>().unwrap());
                }
            }
            let length = length.expect("multipart request must have a known length");
            assert_eq!(
                io::copy(&mut request.take(length), &mut io::sink()).unwrap(),
                length
            );
        }
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let output = sandbox
        .command()
        .arg("convert")
        .arg(input)
        .output()
        .unwrap();
    server.join().unwrap();
    output
}

#[test]
fn convert_stdout_contains_only_the_transcription() {
    // Default INFO diagnostics must not become part of a redirected transcript file.
    let output = convert_speech("200 OK", r#"{"text":"CLI transcript only"}"#);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"CLI transcript only\n", "{output:?}");
    let diagnostics = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostics.contains("Transcribing audio file"));
    assert!(!diagnostics.contains("CLI transcript only"));
}

#[test]
fn silent_conversion_has_no_transcription_or_http_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    // Capture default initialization diagnostics in their own process, without shared tracing state.
    let sandbox = Sandbox::new(&format!(
        "http://url-user:url-password@{}/stt?token=url-token#url-fragment",
        listener.local_addr().unwrap()
    ));
    let input = sandbox.directory.path().join("silence.wav");
    let mut writer = hound::WavWriter::create(
        &input,
        hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for _ in 0..3_200 {
        writer.write_sample(0_i16).unwrap();
    }
    writer.finalize().unwrap();

    let output = sandbox
        .command()
        .arg("convert")
        .arg(input)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(output.stdout, b"\n", "{output:?}");
    let diagnostics = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostics.contains("Transcribing audio file"));
    assert!(diagnostics.contains(&format!("{}/stt", listener.local_addr().unwrap())));
    for secret in [
        "url-user",
        "url-password",
        "token=",
        "url-token",
        "url-fragment",
    ] {
        assert!(
            !diagnostics.contains(secret),
            "leaked {secret}: {diagnostics}"
        );
    }
    listener.set_nonblocking(true).unwrap();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}

#[test]
fn failed_conversion_reports_diagnostics_only_on_stderr() {
    // API errors remain actionable and nonzero without contaminating command-result stdout.
    let output = convert_speech("400 Bad Request", r#"{"error":"stub rejected the model"}"#);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let diagnostics = String::from_utf8(output.stderr).unwrap();
    assert!(diagnostics.contains("Transcribing audio file"));
    assert!(diagnostics.contains("API error 400"));
    assert!(diagnostics.contains("stub rejected the model"));
}
