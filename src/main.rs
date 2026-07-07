mod audio;
mod core;
mod input;
mod local;
mod platform;
mod postprocess;
mod transcriber;

use clap::Parser;
use core::cli::{Cli, Commands, ConfigAction, LocalCommand};
use core::config::AppConfig;
use local::{
    LocalServiceManager, PythonRuntime, dependencies_installed, detect_python_runtime,
    download_model, find_server_file, install_requirements, model_weights_present, setup_venv,
    verify_install,
};
use std::path::PathBuf;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

const LOCAL_MODEL_NAME: &str = "gemma-4-E2B-it";

struct LocalServiceGuard(Option<LocalServiceManager>);

impl LocalServiceGuard {
    fn new(manager: Option<LocalServiceManager>) -> Self {
        Self(manager)
    }
}

impl Drop for LocalServiceGuard {
    fn drop(&mut self) {
        if let Some(manager) = self.0.as_mut() {
            manager.release();
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("viberwhisper=info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    match cli.command {
        None => {
            run_listener()?;
        }
        Some(Commands::Config { action }) => {
            handle_config(action);
        }
        Some(Commands::Local { action }) => {
            handle_local(action)?;
        }
        Some(Commands::Convert { input, output }) => {
            handle_convert(&input, output.as_deref());
        }
    }

    Ok(())
}

fn handle_local(action: LocalCommand) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = AppConfig::load();
    match action {
        LocalCommand::Install => {
            ensure_local_install(&config, true)?;
            println!("Local Gemma runtime is installed.");
            Ok(())
        }
        LocalCommand::Start => {
            config.local_mode = true;
            run_listener_with_config(config)
        }
        LocalCommand::Stop => {
            let paths = local_paths(&config)?;
            let mut manager =
                LocalServiceManager::new(config.local_server_port, paths.model_dir, paths.venv_dir);
            manager.stop();
            println!("Local Gemma service stopped.");
            Ok(())
        }
        LocalCommand::Status => {
            let paths = local_paths(&config)?;
            let manager =
                LocalServiceManager::new(config.local_server_port, paths.model_dir, paths.venv_dir);
            let status = manager.status()?;
            println!("running: {}", status.running);
            println!("port: {}", status.port);
            println!(
                "pid: {}",
                status
                    .pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "n/a".to_string())
            );
            println!(
                "memory: {}",
                status
                    .memory_usage
                    .unwrap_or_else(|| "unavailable".to_string())
            );
            println!("health: {}", status.health);
            Ok(())
        }
    }
}

struct LocalPaths {
    venv_dir: PathBuf,
    model_dir: PathBuf,
}

fn prepare_runtime_config(
    mut config: AppConfig,
) -> Result<(AppConfig, Option<LocalServiceManager>), Box<dyn std::error::Error>> {
    if !config.local_mode {
        return Ok((config, None));
    }

    let paths = ensure_local_install(&config, false)?;
    let mut manager = LocalServiceManager::with_quantization(
        config.local_server_port,
        paths.model_dir,
        paths.venv_dir,
        config.local_quantization.clone(),
    );
    manager.start()?;
    config = apply_local_endpoint_overrides(&config, &manager.base_url());

    Ok((config, Some(manager)))
}

fn ensure_local_install(
    config: &AppConfig,
    install_deps: bool,
) -> Result<LocalPaths, Box<dyn std::error::Error>> {
    let paths = local_paths(config)?;
    let hf_endpoint =
        std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string());
    let runtime = detect_python_runtime()?;

    print_python_runtime(&runtime);

    println!("[local] step 1/4 – python venv");
    setup_venv(&paths.venv_dir)?;

    if install_deps {
        let requirements = local_requirements_path();
        println!("[local] step 2/4 – python dependencies");
        install_requirements(&paths.venv_dir, &requirements)?;
    } else if !dependencies_installed(&paths.venv_dir) {
        return Err(
            "Python dependencies are not installed. Run `viberwhisper local install` first.".into(),
        );
    } else {
        println!("[local] step 2/4 – python dependencies (skipped)");
    }

    if !model_weights_present(&paths.model_dir) {
        println!("[local] step 3/4 – downloading google/gemma-4-E2B-it");
        println!("[local]   set HF_ENDPOINT env var to use a mirror");
    } else {
        println!("[local] step 3/4 – model already present, skipping download");
    }
    download_model(&paths.model_dir, &hf_endpoint)?;

    println!("[local] step 4/4 – verify");
    verify_install(&paths.venv_dir, &paths.model_dir)?;

    Ok(paths)
}

fn print_python_runtime(runtime: &PythonRuntime) {
    let (major, minor) = runtime.version;
    println!(
        "[local] python: {} ({}.{}; require >= 3.10)",
        runtime.python.display(),
        major,
        minor
    );
    match &runtime.uv {
        Some(uv) => println!("[local] package runner: uv ({})", uv.display()),
        None => println!("[local] package runner: system python fallback"),
    }
}

fn local_paths(config: &AppConfig) -> Result<LocalPaths, Box<dyn std::error::Error>> {
    let data_dir = match &config.local_data_dir {
        Some(path) if path.starts_with("~/") => {
            let home = dirs::home_dir().ok_or("could not determine home directory")?;
            home.join(&path[2..])
        }
        Some(path) => PathBuf::from(path),
        None => default_local_data_dir()?,
    };

    Ok(LocalPaths {
        venv_dir: data_dir.join("venv"),
        model_dir: data_dir.join("model"),
    })
}

fn default_local_data_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let home_dir = dirs::home_dir().ok_or("could not determine home directory")?;
    Ok(home_dir.join(".viberwhisper"))
}

fn local_requirements_path() -> PathBuf {
    find_server_file("requirements.txt")
}

fn run_listener() -> Result<(), Box<dyn std::error::Error>> {
    run_listener_with_config(AppConfig::load())
}

/// Keep the raw STT text when post-processing yields nothing or fails —
/// a cleanup step must never lose a successful transcription.
fn resolve_post_processed_text(
    result: Result<String, Box<dyn std::error::Error>>,
    stt_text: String,
) -> String {
    match result {
        Ok(processed) if !processed.is_empty() => processed,
        Ok(_) => {
            warn!("Post-processing returned empty text, using original STT text");
            stt_text
        }
        Err(e) => {
            warn!(error = %e, "Post-processing failed, using original STT text");
            stt_text
        }
    }
}

/// Feeds stable STT text fragments to a post-process session.
///
/// Sessions concatenate pushed fragments verbatim, so this feeder owns the
/// language-appropriate separator and prepends it to every fragment after the
/// first. Opened when recording starts; while the recording continues, stable
/// chunk texts are pushed as they arrive so a preheat LLM session already has
/// the cleanup request in flight by the time the user stops.
struct PostFeed {
    session: Box<dyn postprocess::TextPostProcessorSession>,
    separator: &'static str,
    pushed_any: bool,
}

impl PostFeed {
    fn new(
        session: Box<dyn postprocess::TextPostProcessorSession>,
        separator: &'static str,
    ) -> Self {
        Self {
            session,
            separator,
            pushed_any: false,
        }
    }

    fn push(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.pushed_any && !self.separator.is_empty() {
            self.session
                .push_stable_chunk(&format!("{}{}", self.separator, text));
        } else {
            self.session.push_stable_chunk(text);
        }
        self.pushed_any = true;
    }

    fn finish(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        self.session.finish()
    }
}

fn run_listener_with_config(config: AppConfig) -> Result<(), Box<dyn std::error::Error>> {
    use audio::{AudioRecorder, StopResult};
    use core::orchestrator::{SessionError, SessionMode, SessionOrchestrator};
    use input::hotkey::{HotkeyEvent, HotkeyManager, HotkeySource};
    use input::overlay::OverlayManager;
    use input::tray::TrayManager;
    use input::typer::TextTyper;
    use postprocess::create_post_processor;
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use transcriber::{Transcriber, create_transcriber};

    println!("ViberWhisper - Voice-to-Text Input");
    println!("===================================");
    println!();

    let (config, local_manager) = prepare_runtime_config(config)?;
    let _local_manager = LocalServiceGuard::new(local_manager);
    info!(
        hold_hotkey = %config.hold_hotkey,
        toggle_hotkey = %config.toggle_hotkey,
        model = %config.model,
        language = %config.language.as_deref().unwrap_or("auto"),
        api_url = %config.transcription_api_url,
        max_chunk_duration_secs = config.max_chunk_duration_secs,
        max_chunk_size_bytes = config.max_chunk_size_bytes,
        max_retries = config.max_retries,
        convergence_timeout_secs = config.convergence_timeout_secs,
        post_process_enabled = config.post_process_enabled,
        "Config loaded"
    );

    let hotkey_manager = HotkeyManager::new(&config.hold_hotkey, &config.toggle_hotkey)?;

    #[allow(clippy::arc_with_non_send_sync)]
    let recorder = Arc::new(Mutex::new(AudioRecorder::with_config(
        config.mic_gain,
        config.max_chunk_duration_secs,
        config.max_chunk_size_bytes,
    )?));

    // Build transcriber and wrap in Arc<dyn Transcriber> for orchestrator injection.
    let transcriber: Arc<dyn Transcriber> = Arc::from(create_transcriber(&config)?);

    let post_processor = create_post_processor(&config);

    let orchestrator = SessionOrchestrator::new(
        Arc::clone(&transcriber),
        config.language.clone(),
        Duration::from_secs(config.convergence_timeout_secs),
    );

    #[cfg(target_os = "macos")]
    let typer = platform::macos::MacTyper;
    #[cfg(target_os = "windows")]
    let typer = platform::windows::WindowsTyper;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let typer = input::typer::MockTyper;

    // RefCell so the start/stop closures below and the main loop can share
    // the UI handles on this single thread.
    let tray = RefCell::new(TrayManager::new()?);
    info!("System tray icon started");

    let overlay = RefCell::new(OverlayManager::new()?);
    info!("Floating overlay window started");

    println!(
        "Hold {} to record, release to transcribe.",
        config.hold_hotkey
    );
    println!(
        "Press {} to start recording, press again to stop.",
        config.toggle_hotkey
    );
    println!("Press Ctrl+C to exit.");
    println!();

    // Streaming post-process feed: opened at recording start, fed stable
    // chunk texts while recording, and finished in `finalize`. With the
    // preheat session this keeps the cleanup request warm during recording;
    // with the conservative/noop sessions it simply accumulates.
    let stream_separator = transcriber::merge_separator(config.language.as_deref());
    let post_feed: RefCell<Option<PostFeed>> = RefCell::new(None);
    let new_post_feed = || PostFeed::new(post_processor.start_session(), stream_separator);

    // Finalize a stopped recording: submit the tail chunk (if any) to the orchestrator,
    // then wait for convergence and type (or log) the result.
    let finalize = |stop_result: StopResult| {
        match stop_result {
            StopResult::SingleFile(path) | StopResult::TailChunk(path) => {
                orchestrator.on_chunk_ready(path);
            }
            StopResult::ChunkFiles(paths) => {
                for path in paths {
                    orchestrator.on_chunk_ready(path);
                }
            }
            StopResult::ChunksOnly => {
                debug!("All audio was flushed to background chunks during recording; no tail");
            }
        }

        // The feed (if any) already received the stable prefix during
        // recording; on success only the unconsumed remainder is pushed.
        // On failure it is discarded — the raw partial text is typed as-is.
        let feed = post_feed.borrow_mut().take();

        match orchestrator.stop_session() {
            Ok(output) => {
                if output.full_text.is_empty() {
                    info!("Transcription returned empty text");
                    return;
                }
                let mut feed = feed.unwrap_or_else(&new_post_feed);
                feed.push(&output.unconsumed_text);
                let text = resolve_post_processed_text(feed.finish(), output.full_text);
                info!(text = %text, "Typing transcribed text");
                if let Err(e) = typer.type_text(&text) {
                    error!(error = %e, "Failed to type text");
                }
            }
            Err(SessionError::NoChunks) => {
                warn!("No audio chunks to transcribe (recording too short?)");
            }
            Err(SessionError::PartialFailure {
                errors,
                partial_text,
            }) => {
                error!(
                    failed_chunks = errors.len(),
                    "Partial transcription failure; typing available text"
                );
                if !partial_text.is_empty()
                    && let Err(e) = typer.type_text(&partial_text)
                {
                    error!(error = %e, "Failed to type partial text");
                }
            }
            Err(SessionError::ConvergenceTimeout {
                pending_count,
                partial_text,
            }) => {
                warn!(
                    pending_count = pending_count,
                    "Convergence timeout; typing available partial text"
                );
                if !partial_text.is_empty()
                    && let Err(e) = typer.type_text(&partial_text)
                {
                    error!(error = %e, "Failed to type partial text");
                }
            }
        }
    };

    // Shared start/stop paths for the hold hotkey, toggle hotkey, and overlay
    // click. UI state (tray + overlay) always tracks the recorder state.
    let set_recording_ui = |on: bool| {
        tray.borrow_mut().set_recording(on);
        overlay.borrow_mut().set_recording(on);
    };

    let start_recording = |mode: SessionMode, trigger: &str| {
        let mut rec = recorder.lock().unwrap();
        match rec.start_recording() {
            Ok(()) => {
                orchestrator.start_session(mode);
                *post_feed.borrow_mut() = Some(new_post_feed());
                info!(trigger = trigger, mode = ?mode, "Recording started");
                set_recording_ui(true);
            }
            Err(e) => error!(error = %e, "Failed to start recording"),
        }
    };

    let stop_recording = |trigger: &str| {
        info!(trigger = trigger, "Stopping recording");
        let stop_result = recorder.lock().unwrap().stop_recording();
        set_recording_ui(false);
        match stop_result {
            Ok(result) => finalize(result),
            Err(e) => error!(error = %e, "Failed to stop recording"),
        }
    };

    let mut counter = 0;
    loop {
        if let Some(event) = hotkey_manager.check_event() {
            match event {
                HotkeyEvent::Pressed(HotkeySource::Hold) => {
                    start_recording(SessionMode::Hold, "hold-press");
                }
                HotkeyEvent::Released(HotkeySource::Hold) => {
                    stop_recording("hold-release");
                }
                HotkeyEvent::Pressed(HotkeySource::Toggle) => {
                    if recorder.lock().unwrap().is_recording() {
                        stop_recording("toggle-press");
                    } else {
                        start_recording(SessionMode::Toggle, "toggle-press");
                    }
                }
                HotkeyEvent::Released(HotkeySource::Toggle) => {}
            }
        }

        // Check overlay click (acts like toggle hotkey)
        if overlay.borrow_mut().check_click() {
            if recorder.lock().unwrap().is_recording() {
                stop_recording("overlay-click");
            } else {
                start_recording(SessionMode::Toggle, "overlay-click");
            }
        }

        // Poll for in-recording chunks from the recorder and forward to the orchestrator.
        {
            let chunk_path = recorder.lock().unwrap().take_ready_chunk();
            if let Some(path) = chunk_path {
                info!(path = %path, "Ready chunk detected, forwarding to orchestrator");
                orchestrator.on_chunk_ready(path);
            }
        }

        // Feed newly stable chunk texts to the post-process session so a
        // preheat LLM request is already in flight before recording stops.
        {
            let stable_texts = orchestrator.take_stable_texts();
            if !stable_texts.is_empty()
                && let Some(feed) = post_feed.borrow_mut().as_mut()
            {
                for text in &stable_texts {
                    debug!(
                        text_len = text.len(),
                        "Feeding stable chunk text to post-processor"
                    );
                    feed.push(text);
                }
            }
        }

        if tray.borrow().check_exit() {
            info!("User clicked exit from tray");
            break Ok(());
        }

        // Wraps every ~3s tick window; a plain increment would overflow an
        // i32 (panicking in debug builds) after ~248 days of uptime.
        counter = (counter + 1) % 300;
        if counter == 0 {
            let status = if recorder.lock().unwrap().is_recording() {
                "recording"
            } else {
                "idle"
            };
            debug!(
                status = status,
                hold_hotkey = %config.hold_hotkey,
                toggle_hotkey = %config.toggle_hotkey,
                "Heartbeat"
            );
        }

        overlay.borrow_mut().update();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn handle_config(action: ConfigAction) {
    use crate::core::config::AppConfig;

    let mut config = AppConfig::load();

    match action {
        ConfigAction::List => {
            println!("{:<25} Value", "Key");
            println!("{}", "-".repeat(60));
            for key in AppConfig::field_keys() {
                let value = config
                    .get_field(key)
                    .unwrap_or_else(|| "(not set)".to_string());
                println!("{:<25} {}", key, value);
            }
        }

        ConfigAction::Get { key } => match config.get_field(&key) {
            Some(value) => println!("{}", value),
            None => {
                eprintln!("Error: unknown config key '{}'", key);
                std::process::exit(1);
            }
        },

        ConfigAction::Set { key, value } => match config.set_field(&key, &value) {
            Ok(()) => {
                if let Err(e) = config.save() {
                    eprintln!("Failed to save config: {}", e);
                    std::process::exit(1);
                }
                println!("Set {} = {}", key, value);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        },
    }
}

fn handle_convert(input: &str, output: Option<&str>) {
    use postprocess::create_post_processor;
    use transcriber::{Transcriber, create_transcriber};

    println!("Transcribing: {}", input);

    let config = AppConfig::load();
    let (config, local_manager) = match prepare_runtime_config(config) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Failed to prepare runtime: {}", e);
            std::process::exit(1);
        }
    };
    let _local_manager = LocalServiceGuard::new(local_manager);
    let transcriber: Box<dyn Transcriber> = match create_transcriber(&config) {
        Ok(transcriber) => transcriber,
        Err(e) => {
            eprintln!("Failed to initialize transcriber: {}", e);
            std::process::exit(1);
        }
    };
    let post_processor = create_post_processor(&config);

    match transcriber.transcribe(input) {
        Ok(stt_text) => {
            let text = resolve_post_processed_text(post_processor.process(&stt_text), stt_text);
            match output {
                Some(path) => {
                    if let Err(e) = std::fs::write(path, &text) {
                        eprintln!("Failed to write file: {}", e);
                        std::process::exit(1);
                    }
                    println!("Saved to: {}", path);
                }
                None => println!("{}", text),
            }
        }
        Err(e) => {
            eprintln!("Transcription failed: {}", e);
            std::process::exit(1);
        }
    }
}

fn apply_local_endpoint_overrides(config: &AppConfig, base_url: &str) -> AppConfig {
    let mut local = config.clone();
    local.api_key = Some("local".to_string());
    local.transcription_api_url = format!("{base_url}/v1/audio/transcriptions");
    if local.post_process_enabled {
        local.post_process_api_key = Some("local".to_string());
        local.post_process_api_url = Some(format!("{base_url}/v1/chat/completions"));
        local.post_process_model = Some(LOCAL_MODEL_NAME.to_string());
    }
    local
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::core::config::AppConfig;
    use audio::AudioRecorder;
    use transcriber::{MockTranscriber, Transcriber};

    #[test]
    fn test_resolve_post_processed_text_prefers_non_empty_result() {
        let text = resolve_post_processed_text(Ok("cleaned".to_string()), "raw".to_string());
        assert_eq!(text, "cleaned");
    }

    #[test]
    fn test_resolve_post_processed_text_keeps_stt_on_empty_result() {
        let text = resolve_post_processed_text(Ok(String::new()), "raw".to_string());
        assert_eq!(text, "raw");
    }

    #[test]
    fn test_resolve_post_processed_text_keeps_stt_on_error() {
        let text = resolve_post_processed_text(Err("llm down".into()), "raw".to_string());
        assert_eq!(text, "raw");
    }

    #[test]
    fn test_post_feed_inserts_separator_between_fragments() {
        use postprocess::{NoopPostProcessor, TextPostProcessor};

        let mut feed = PostFeed::new(NoopPostProcessor.start_session(), " ");
        feed.push("hello");
        feed.push("");
        feed.push("world");
        assert_eq!(feed.finish().unwrap(), "hello world");
    }

    #[test]
    fn test_post_feed_zh_concatenates_without_separator() {
        use postprocess::{NoopPostProcessor, TextPostProcessor};

        let mut feed = PostFeed::new(NoopPostProcessor.start_session(), "");
        feed.push("你好");
        feed.push("世界");
        assert_eq!(feed.finish().unwrap(), "你好世界");
    }

    #[test]
    fn test_audio_module_loads() {
        let audio_result = AudioRecorder::new(1.0);
        assert!(audio_result.is_ok());
    }

    #[test]
    fn test_full_pipeline_mock() {
        use input::typer::{MockTyper, TextTyper};
        let transcriber = MockTranscriber;
        let typer = MockTyper;
        let text = transcriber.transcribe("fake.wav").unwrap();
        assert!(typer.type_text(&text).is_ok());
    }

    #[test]
    fn test_orchestrator_integration_single_chunk() {
        use self::core::orchestrator::{SessionMode, SessionOrchestrator};
        use std::sync::Arc;
        use std::time::Duration;

        let t: Arc<dyn Transcriber> = Arc::new(MockTranscriber);
        let orch = SessionOrchestrator::new(t, Some("en".to_string()), Duration::from_secs(5));

        orch.start_session(SessionMode::Hold);
        // MockTranscriber ignores the path, so a non-existent path is fine.
        orch.on_chunk_ready("fake_chunk.wav".to_string());
        let result = orch.stop_session();

        assert!(result.is_ok(), "Expected Ok, got {:?}", result);
        assert!(!result.unwrap().full_text.is_empty());
    }

    #[test]
    fn test_orchestrator_no_chunks() {
        use self::core::orchestrator::{SessionError, SessionMode, SessionOrchestrator};
        use std::sync::Arc;
        use std::time::Duration;

        let t: Arc<dyn Transcriber> = Arc::new(MockTranscriber);
        let orch = SessionOrchestrator::new(t, None, Duration::from_secs(5));

        orch.start_session(SessionMode::Toggle);
        let result = orch.stop_session();
        assert!(matches!(result, Err(SessionError::NoChunks)));
    }

    #[test]
    fn test_apply_local_endpoint_overrides_enabled_post_process() {
        let config = AppConfig {
            local_server_port: 17265,
            post_process_enabled: true,
            post_process_model: Some("gpt-4o-mini".to_string()),
            ..Default::default()
        };

        let local = apply_local_endpoint_overrides(&config, "http://127.0.0.1:17265");

        assert_eq!(
            local.transcription_api_url,
            "http://127.0.0.1:17265/v1/audio/transcriptions"
        );
        assert_eq!(
            local.post_process_api_url.as_deref(),
            Some("http://127.0.0.1:17265/v1/chat/completions")
        );
        assert!(local.post_process_enabled);
        assert_eq!(local.post_process_model.as_deref(), Some("gemma-4-E2B-it"));
        assert_eq!(local.api_key.as_deref(), Some("local"));
        assert_eq!(local.post_process_api_key.as_deref(), Some("local"));
    }

    #[test]
    fn test_apply_local_endpoint_overrides_keeps_post_process_disabled() {
        let config = AppConfig {
            local_server_port: 17265,
            post_process_enabled: false,
            post_process_model: Some("gpt-4o-mini".to_string()),
            ..Default::default()
        };

        let local = apply_local_endpoint_overrides(&config, "http://127.0.0.1:17265");

        assert_eq!(
            local.transcription_api_url,
            "http://127.0.0.1:17265/v1/audio/transcriptions"
        );
        assert!(!local.post_process_enabled);
        assert_eq!(local.post_process_model.as_deref(), Some("gpt-4o-mini"));
        assert_eq!(local.post_process_api_url, None);
        assert_eq!(local.post_process_api_key, None);
        assert_eq!(local.api_key.as_deref(), Some("local"));
    }
}
