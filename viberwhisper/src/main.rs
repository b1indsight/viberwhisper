mod audio;
mod cli;
mod config;
mod hotkey;
mod transcriber;
mod tray;
mod typer;

use clap::Parser;
use cli::{Cli, Commands, ConfigAction};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        None => {
            run_listener()?;
        }
        Some(Commands::Config { action }) => {
            handle_config(action);
        }
        Some(Commands::Convert { input, output }) => {
            handle_convert(&input, output.as_deref());
        }
    }

    Ok(())
}

/// 原有的语音监听主循环
fn run_listener() -> Result<(), Box<dyn std::error::Error>> {
    use audio::AudioRecorder;
    use config::AppConfig;
    use hotkey::{HotkeyEvent, HotkeyManager, HotkeySource};
    use std::sync::{Arc, Mutex};
    use transcriber::{GroqTranscriber, MockTranscriber, Transcriber};
    use tray::TrayManager;
    use typer::{TextTyper, WindowsTyper};

    println!("ViberWhisper - Voice-to-Text Input");
    println!("===================================");
    println!();

    let config = AppConfig::load();
    println!(
        "[Config] Hold热键: {}  Toggle热键: {}  模型: {}  语言: {}",
        config.hold_hotkey,
        config.toggle_hotkey,
        config.model,
        config.language.as_deref().unwrap_or("auto"),
    );
    println!();

    let hotkey_manager = HotkeyManager::new(&config.hold_hotkey, &config.toggle_hotkey)?;
    let recorder = Arc::new(Mutex::new(AudioRecorder::new(config.mic_gain)?));
    let transcriber: Box<dyn Transcriber> = match GroqTranscriber::from_config(&config) {
        Ok(t) => {
            println!("使用 Groq Whisper 进行语音识别");
            Box::new(t)
        }
        Err(e) => {
            eprintln!("警告: {} - 回退到 Mock 模式", e);
            Box::new(MockTranscriber)
        }
    };
    let typer = WindowsTyper;

    let mut tray = TrayManager::new()?;
    println!("[Tray] 系统托盘图标已启动");

    println!("Hold {} to record, release to transcribe.", config.hold_hotkey);
    println!(
        "Press {} to start recording, press again to stop.",
        config.toggle_hotkey
    );
    println!("Press Ctrl+C to exit.");
    println!();

    // 停止录音并转录的辅助闭包
    let stop_and_transcribe = |rec: &mut AudioRecorder| {
        match rec.stop_recording() {
            Ok(wav_path) => {
                println!("Recording saved: {}", wav_path);
                match transcriber.transcribe(&wav_path) {
                    Ok(text) => {
                        if let Err(e) = typer.type_text(&text) {
                            eprintln!("Failed to type text: {}", e);
                        }
                    }
                    Err(e) => eprintln!("Transcription failed: {}", e),
                }
            }
            Err(e) => eprintln!("Failed to stop recording: {}", e),
        }
    };

    let mut counter = 0;
    loop {
        if let Some(event) = hotkey_manager.check_event() {
            match event {
                // Hold 模式：按下开始录音
                HotkeyEvent::Pressed(HotkeySource::Hold) => {
                    println!("{} pressed (hold), starting recording...", config.hold_hotkey);
                    let mut rec = recorder.lock().unwrap();
                    match rec.start_recording() {
                        Ok(()) => {
                            println!("Recording started.");
                            tray.set_recording(true);
                        }
                        Err(e) => eprintln!("Failed to start recording: {}", e),
                    }
                }
                // Hold 模式：松开停止录音并转录
                HotkeyEvent::Released(HotkeySource::Hold) => {
                    println!(
                        "{} released (hold), stopping recording...",
                        config.hold_hotkey
                    );
                    let mut rec = recorder.lock().unwrap();
                    stop_and_transcribe(&mut rec);
                    tray.set_recording(false);
                }
                // Toggle 模式：按下切换录音状态
                HotkeyEvent::Pressed(HotkeySource::Toggle) => {
                    let mut rec = recorder.lock().unwrap();
                    if rec.is_recording() {
                        println!(
                            "{} pressed (toggle), stopping recording...",
                            config.toggle_hotkey
                        );
                        stop_and_transcribe(&mut rec);
                        tray.set_recording(false);
                    } else {
                        println!(
                            "{} pressed (toggle), starting recording...",
                            config.toggle_hotkey
                        );
                        match rec.start_recording() {
                            Ok(()) => {
                                println!("Recording started.");
                                tray.set_recording(true);
                            }
                            Err(e) => eprintln!("Failed to start recording: {}", e),
                        }
                    }
                }
                // Toggle 模式不需要处理 Released 事件
                HotkeyEvent::Released(HotkeySource::Toggle) => {}
            }
        }

        // 检查托盘菜单退出
        if tray.check_exit() {
            println!("[Tray] 用户点击退出");
            break Ok(());
        }

        counter += 1;
        if counter % 300 == 0 {
            let status = if recorder.lock().unwrap().is_recording() {
                "Recording..."
            } else {
                "Idle"
            };
            println!(
                "[Heartbeat] {} | hold={} toggle={}",
                status, config.hold_hotkey, config.toggle_hotkey
            );
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// 处理 config 子命令
fn handle_config(action: ConfigAction) {
    use config::AppConfig;

    let mut config = AppConfig::load();

    match action {
        ConfigAction::List => {
            println!("{:<15} {}", "配置项", "当前值");
            println!("{}", "-".repeat(50));
            for key in &[
                "model",
                "hold_hotkey",
                "toggle_hotkey",
                "language",
                "prompt",
                "temperature",
                "mic_gain",
                "groq_api_key",
            ] {
                let value = config
                    .get_field(key)
                    .unwrap_or_else(|| "（未设置）".to_string());
                println!("{:<15} {}", key, value);
            }
        }

        ConfigAction::Get { key } => match config.get_field(&key) {
            Some(value) => println!("{}", value),
            None => {
                eprintln!("错误：未知配置项 '{}'", key);
                std::process::exit(1);
            }
        },

        ConfigAction::Set { key, value } => match config.set_field(&key, &value) {
            Ok(()) => {
                if let Err(e) = config.save() {
                    eprintln!("保存配置失败: {}", e);
                    std::process::exit(1);
                }
                println!("已设置 {} = {}", key, value);
            }
            Err(e) => {
                eprintln!("错误：{}", e);
                std::process::exit(1);
            }
        },
    }
}

/// 处理 convert 子命令
fn handle_convert(input: &str, output: Option<&str>) {
    use config::AppConfig;
    use transcriber::{GroqTranscriber, MockTranscriber, Transcriber};

    println!("正在转录: {}", input);

    let config = AppConfig::load();

    let transcriber: Box<dyn Transcriber> = match GroqTranscriber::from_config(&config) {
        Ok(t) => Box::new(t),
        Err(e) => {
            eprintln!("警告：无法初始化 Groq（{}），使用 Mock 转录器", e);
            Box::new(MockTranscriber)
        }
    };

    match transcriber.transcribe(input) {
        Ok(text) => match output {
            Some(path) => {
                if let Err(e) = std::fs::write(path, &text) {
                    eprintln!("写入文件失败: {}", e);
                    std::process::exit(1);
                }
                println!("已保存到: {}", path);
            }
            None => println!("{}", text),
        },
        Err(e) => {
            eprintln!("转录失败: {}", e);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use audio::AudioRecorder;
    use transcriber::{MockTranscriber, Transcriber};

    #[test]
    fn test_audio_module_loads() {
        let audio_result = AudioRecorder::new(1.0);
        assert!(audio_result.is_ok());
    }

    #[test]
    fn test_full_pipeline_mock() {
        use typer::{MockTyper, TextTyper};
        let transcriber = MockTranscriber;
        let typer = MockTyper;
        let text = transcriber.transcribe("fake.wav").unwrap();
        assert!(typer.type_text(&text).is_ok());
    }
}
