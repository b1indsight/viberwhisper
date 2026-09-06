//! First-run and on-demand setup orchestration.
//!
//! Native dialogs stay behind `SetupUi`; configuration, audio, and HTTP behavior remain owned by
//! their existing modules and are injected at the workflow boundaries for deterministic tests.

mod hotkey;

use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result as AnyhowResult, anyhow};
use rdev::EventType;
use tinyfiledialogs::{MessageBoxIcon, YesNo};

use super::config_context;
use crate::audio::{AudioRecorder, RecorderStartOutcome, RecorderStopOutcome};
use crate::core::config::{
    ConfigDocument, ConfigStore, EnvironmentSecretSource, InferenceProfile, SecretSource,
};
use crate::postprocess::PostProcessor;
use crate::runtime_config::{self, ListenerConfig, ProfileSelection};
use crate::session::SessionId;
use crate::transcriber::{ApiTranscriber, Transcriber};
use crate::{audio, text};

const TITLE: &str = "ViberWhisper 设置";
const DEFAULT_POST_PROCESS_URL: &str = "https://api.openai.com/v1/chat/completions";
const DEFAULT_POST_PROCESS_MODEL: &str = "gpt-4o-mini";
const HOTKEY_CAPTURE_HELPER_ENV: &str = "VIBERWHISPER_CAPTURE_HOTKEY";
const HOTKEY_CAPTURE_EXIT_TIMEOUT: Duration = Duration::from_secs(1);

pub(super) fn run_hotkey_capture_helper_if_requested() -> AnyhowResult<bool> {
    if hotkey::run_helper_if_requested()? {
        return Ok(true);
    }
    if std::env::var_os(HOTKEY_CAPTURE_HELPER_ENV).is_none() {
        return Ok(false);
    }

    let result = rdev::listen(|event| {
        let EventType::KeyPress(key) = event.event_type else {
            return;
        };
        let Some(name) = crate::input::hotkey::canonical_key_name(key) else {
            return;
        };
        let mut stdout = std::io::stdout().lock();
        let exit_code = if writeln!(stdout, "{name}")
            .and_then(|()| stdout.flush())
            .is_ok()
        {
            0
        } else {
            2
        };
        std::process::exit(exit_code);
    });

    match result {
        Ok(()) => Err(anyhow!("hotkey listener stopped before capturing a key")),
        Err(error) => Err(anyhow!("could not start hotkey capture: {error:?}")),
    }
}

pub(super) fn listener_config() -> AnyhowResult<Option<ListenerConfig>> {
    let store = ConfigStore::discover()?;
    let (initial, reason) = match store.load() {
        Ok(None) => (
            ConfigDocument::default(),
            "尚未找到配置文件。是否现在运行配置向导？\n选择“否”将在本次启动使用内置默认值。"
                .to_string(),
        ),
        Ok(Some(document)) => match resolve_document(&store, &document) {
            Ok(config) => return Ok(Some(config)),
            Err(error) => (
                document,
                format!(
                    "当前配置无法启动监听器：\n{}\n\n是否现在修复？\n选择“否”将在本次启动使用内置默认值。",
                    safe_dialog_text(&error.to_string())
                ),
            ),
        },
        Err(error) => (
            ConfigDocument::default(),
            format!(
                "配置文件无法读取：\n{}\n\n是否从默认值开始修复？\n选择“否”将在本次启动使用内置默认值，且不会覆盖原文件。",
                safe_dialog_text(&error.to_string())
            ),
        ),
    };

    let mut ui = NativeSetupUi;
    if !ui.confirm(&reason, true) {
        return Ok(Some(resolve_document(&store, &ConfigDocument::default())?));
    }

    let mut verifier = NativeVerifier::new(&store)?;
    match run_wizard(
        &store,
        initial,
        audio::recorder::input_device_names(),
        &mut ui,
        &mut verifier,
    )? {
        WizardOutcome::Saved(document) => Ok(Some(resolve_document(&store, &document)?)),
        WizardOutcome::Cancelled => Ok(None),
    }
}

pub(super) fn run_explicit() -> AnyhowResult<()> {
    let store = ConfigStore::discover()?;
    let initial = match store.load() {
        Ok(Some(document)) => document,
        Ok(None) => ConfigDocument::default(),
        Err(error) => {
            let mut ui = NativeSetupUi;
            ui.message(&format!(
                "现有配置无法读取，将从默认值开始。原文件只有在最终确认保存后才会被替换。\n\n{}",
                safe_dialog_text(&error.to_string())
            ));
            ConfigDocument::default()
        }
    };
    let mut ui = NativeSetupUi;
    let mut verifier = NativeVerifier::new(&store)?;
    if let WizardOutcome::Saved(document) = run_wizard(
        &store,
        initial,
        audio::recorder::input_device_names(),
        &mut ui,
        &mut verifier,
    )? {
        resolve_document(&store, &document)?;
    }
    Ok(())
}

fn resolve_document(
    store: &ConfigStore,
    document: &ConfigDocument,
) -> AnyhowResult<ListenerConfig> {
    let (config_dir, home_dir) = config_context(store)?;
    Ok(runtime_config::resolve_listener(
        document,
        &EnvironmentSecretSource,
        ProfileSelection::Configured,
        &config_dir,
        &home_dir,
    )?)
}

enum WizardOutcome {
    Saved(Box<ConfigDocument>),
    Cancelled,
}

trait SetupUi {
    fn confirm(&mut self, message: &str, default_yes: bool) -> bool;
    fn input(&mut self, message: &str, default: &str) -> Option<String>;
    fn password(&mut self, message: &str) -> Option<String>;
    fn capture_hotkey(&mut self, message: &str) -> Result<Option<String>, String>;
    fn message(&mut self, message: &str);
}

struct NativeSetupUi;

impl SetupUi for NativeSetupUi {
    fn confirm(&mut self, message: &str, default_yes: bool) -> bool {
        tinyfiledialogs::message_box_yes_no(
            TITLE,
            message,
            MessageBoxIcon::Question,
            if default_yes { YesNo::Yes } else { YesNo::No },
        ) == YesNo::Yes
    }

    fn input(&mut self, message: &str, default: &str) -> Option<String> {
        tinyfiledialogs::input_box(TITLE, message, default)
    }

    fn password(&mut self, message: &str) -> Option<String> {
        tinyfiledialogs::password_box(TITLE, message)
    }

    fn capture_hotkey(&mut self, message: &str) -> Result<Option<String>, String> {
        capture_hotkey_with_helper(message)
    }

    fn message(&mut self, message: &str) {
        tinyfiledialogs::message_box_ok(TITLE, message, MessageBoxIcon::Info);
    }
}

fn capture_hotkey_with_helper(message: &str) -> Result<Option<String>, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("无法定位当前程序：{error}"))?;
    let mut command = Command::new(executable);
    command
        .env(HOTKEY_CAPTURE_HELPER_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动热键捕获：{error}"))?;

    tinyfiledialogs::message_box_ok(TITLE, message, MessageBoxIcon::Info);
    let deadline = Instant::now() + HOTKEY_CAPTURE_EXIT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
            Err(error) => return Err(format!("等待热键输入失败：{error}")),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("读取热键输入失败：{error}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("热键捕获失败：{}", safe_dialog_text(detail.trim())));
    }
    let name = String::from_utf8(output.stdout)
        .map_err(|error| format!("热键名称不是有效文本：{error}"))?;
    let name = name.trim();
    if crate::input::hotkey::parse_key(name).is_none() {
        return Err("热键捕获返回了无法识别的按键。".to_string());
    }
    Ok(Some(name.to_string()))
}

trait SetupVerifier {
    fn verify(
        &mut self,
        document: &ConfigDocument,
        ui: &mut dyn SetupUi,
    ) -> Result<VerificationResult, String>;
}

struct VerificationResult {
    raw: String,
    final_text: String,
}

struct NativeVerifier {
    config_dir: std::path::PathBuf,
    home_dir: std::path::PathBuf,
}

impl NativeVerifier {
    fn new(store: &ConfigStore) -> AnyhowResult<Self> {
        let (config_dir, home_dir) = config_context(store)?;
        Ok(Self {
            config_dir,
            home_dir,
        })
    }
}

impl SetupVerifier for NativeVerifier {
    fn verify(
        &mut self,
        document: &ConfigDocument,
        _ui: &mut dyn SetupUi,
    ) -> Result<VerificationResult, String> {
        let config = runtime_config::resolve_listener(
            document,
            &EnvironmentSecretSource,
            ProfileSelection::Configured,
            &self.config_dir,
            &self.home_dir,
        )
        .map_err(|error| error.to_string())?;
        let transcriber =
            ApiTranscriber::new(config.backend.transcriber).map_err(|error| error.to_string())?;
        let post_processor = PostProcessor::new(config.backend.post_process);
        let mut recorder = AudioRecorder::with_config(&config.audio, |_| {});
        let session_id = SessionId(1);
        let mut hotkeys = hotkey::VerificationListener::spawn(&document.input)?;

        loop {
            match hotkeys.receive(Duration::from_millis(50))? {
                Some(hotkey::VerificationAction::Start) => break,
                Some(hotkey::VerificationAction::Stop) | None => {}
            }
        }
        match recorder.start_recording(session_id) {
            RecorderStartOutcome::Started { .. } => {}
            RecorderStartOutcome::Failed { error, .. } => return Err(error),
            RecorderStartOutcome::AlreadyRecording { .. } => {
                return Err("测试录音器已处于录音状态".to_string());
            }
        }

        let mut chunks = Vec::new();
        loop {
            while let Some(ready) = recorder.take_ready_chunk() {
                chunks.push(ready.chunk);
            }
            match hotkeys.receive(Duration::from_millis(50)) {
                Ok(Some(hotkey::VerificationAction::Stop)) => break,
                Ok(Some(hotkey::VerificationAction::Start)) | Ok(None) => {}
                Err(error) => {
                    let _ = recorder.cancel_recording(session_id);
                    return Err(error);
                }
            }
        }
        match recorder.stop_recording(session_id) {
            RecorderStopOutcome::Stopped {
                chunks: final_chunks,
                warning,
                ..
            } => {
                if let Some(warning) = warning {
                    return Err(warning);
                }
                chunks.extend(final_chunks);
            }
            RecorderStopOutcome::StillRecording { error, .. } => return Err(error),
            RecorderStopOutcome::NotRecording { .. } => {
                return Err("测试录音未启动".to_string());
            }
        }
        hotkeys.finish()?;
        if chunks.is_empty() {
            return Err("没有录到音频数据".to_string());
        }

        let mut texts = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            texts.push(
                transcriber
                    .transcribe(&chunk)
                    .map_err(|error| error.to_string())?,
            );
        }
        let raw = text::merge_texts(&texts, document.transcription.language.as_deref());
        if raw.trim().is_empty() {
            return Err("转写服务返回了空文本".to_string());
        }
        let final_text = post_processor
            .process(&raw)
            .map_err(|error| error.to_string())?;
        Ok(VerificationResult { raw, final_text })
    }
}

fn run_wizard(
    store: &ConfigStore,
    mut document: ConfigDocument,
    device_names: Result<Vec<String>, String>,
    ui: &mut dyn SetupUi,
    verifier: &mut dyn SetupVerifier,
) -> AnyhowResult<WizardOutcome> {
    document.inference.active = InferenceProfile::Api;
    let Some(url) = required_input(
        ui,
        "请输入 OpenAI 兼容的 STT API 地址",
        &document.inference.api.transcription.api_url,
    ) else {
        return Ok(WizardOutcome::Cancelled);
    };
    document.inference.api.transcription.api_url = url;

    let Some(model) = required_input(
        ui,
        "请输入 STT 模型名称",
        &document.inference.api.transcription.model,
    ) else {
        return Ok(WizardOutcome::Cancelled);
    };
    document.inference.api.transcription.model = model;

    let stt_key_message = if EnvironmentSecretSource
        .get("TRANSCRIPTION_API_KEY")
        .is_some()
    {
        "已检测到 TRANSCRIPTION_API_KEY 环境变量。留空即可使用它，输入新值只会保存为备用磁盘密钥。"
    } else {
        "请输入 STT API Key。留空会保留已有磁盘密钥，也可使用 TRANSCRIPTION_API_KEY 环境变量。"
    };
    let Some(key) = ui.password(stt_key_message) else {
        return Ok(WizardOutcome::Cancelled);
    };
    if !key.is_empty() {
        document.set_transcription_api_key(Some(key));
    }

    document.post_process.enabled = ui.confirm("是否启用 LLM 文本整理？", false);
    if document.post_process.enabled {
        let current_url = document
            .inference
            .api
            .post_process
            .api_url
            .as_deref()
            .unwrap_or(DEFAULT_POST_PROCESS_URL);
        let Some(url) = required_input(ui, "请输入 LLM Chat Completions API 地址", current_url)
        else {
            return Ok(WizardOutcome::Cancelled);
        };
        document.inference.api.post_process.api_url = Some(url);

        let current_model = document
            .inference
            .api
            .post_process
            .model
            .as_deref()
            .unwrap_or(DEFAULT_POST_PROCESS_MODEL);
        let Some(model) = required_input(ui, "请输入 LLM 模型名称", current_model) else {
            return Ok(WizardOutcome::Cancelled);
        };
        document.inference.api.post_process.model = Some(model);
        let post_key_message = if EnvironmentSecretSource
            .get("POST_PROCESS_API_KEY")
            .is_some()
        {
            "已检测到 POST_PROCESS_API_KEY 环境变量。留空即可使用它，输入新值只会保存为备用磁盘密钥。"
        } else {
            "请输入 LLM API Key。留空会保留已有磁盘密钥，也可使用 POST_PROCESS_API_KEY 环境变量。"
        };
        let Some(key) = ui.password(post_key_message) else {
            return Ok(WizardOutcome::Cancelled);
        };
        if !key.is_empty() {
            document.set_post_process_api_key(Some(key));
        }
    }

    if configure_hotkeys(&mut document, ui).is_err() {
        return Ok(WizardOutcome::Cancelled);
    }

    document.audio.input_device =
        match choose_input_device(ui, document.audio.input_device.as_deref(), device_names) {
            Ok(device) => device,
            Err(()) => return Ok(WizardOutcome::Cancelled),
        };

    if let Err(error) = resolve_document(store, &document) {
        ui.message(&format!(
            "配置仍有问题，未保存：\n{}",
            safe_dialog_text(&error.to_string())
        ));
        return Ok(WizardOutcome::Cancelled);
    }

    let verification_prompt = format!(
        "是否现在使用热键录制一段语音并测试转写？\n\n按住录音：{}（按下开始，松开停止）\n切换录音：{}（按一次开始，再按一次停止）\n\n选择“是”后，此窗口会关闭，程序将等待热键。开始录音后请说“测试”，再用对应方式结束录音；识别完成后会显示结果。",
        display_binding(&document.input.hold_hotkey),
        display_binding(&document.input.toggle_hotkey)
    );
    if ui.confirm(&verification_prompt, true) {
        loop {
            match verifier.verify(&document, ui) {
                Ok(result) => {
                    ui.message(&format!(
                        "原始转写：\n{}\n\n最终文本：\n{}",
                        safe_dialog_text(&result.raw),
                        safe_dialog_text(&result.final_text)
                    ));
                    break;
                }
                Err(error) => {
                    let retry = ui.confirm(
                        &format!(
                            "测试失败：\n{}\n\n是否重试？选择“否”可以继续保存未经验证的配置。",
                            safe_dialog_text(&error)
                        ),
                        true,
                    );
                    if !retry {
                        break;
                    }
                }
            }
        }
    }

    if !ui.confirm(
        "API Key 若写入配置文件会以本机明文保存。是否确认保存当前配置？",
        true,
    ) {
        return Ok(WizardOutcome::Cancelled);
    }
    store.save(&document)?;
    ui.message(&format!(
        "配置已保存到：\n{}",
        safe_dialog_text(&store.path().display().to_string())
    ));
    Ok(WizardOutcome::Saved(Box::new(document)))
}

fn required_input(ui: &mut dyn SetupUi, message: &str, default: &str) -> Option<String> {
    loop {
        let value = ui.input(message, &safe_dialog_text(default))?;
        if !value.trim().is_empty() {
            return Some(value.trim().to_string());
        }
        ui.message("该项不能为空。");
    }
}

fn configure_hotkeys(document: &mut ConfigDocument, ui: &mut dyn SetupUi) -> Result<(), ()> {
    let current_valid = crate::platform::validate_hotkeys(&document.input).is_ok();
    let summary = format!(
        "当前录音热键：\n按住录音：{}\n切换录音：{}\n\n是否保留当前设置？",
        display_binding(&document.input.hold_hotkey),
        display_binding(&document.input.toggle_hotkey)
    );
    if current_valid && ui.confirm(&summary, true) {
        return Ok(());
    }
    if !current_valid {
        ui.message("当前热键设置无效，请重新设置。");
    }

    loop {
        let hold_hotkey = capture_binding(ui, "按住录音", &document.input.hold_hotkey)?;
        let toggle_hotkey = capture_binding(ui, "切换录音", &document.input.toggle_hotkey)?;
        let candidate = crate::core::config::InputSection {
            hold_hotkey,
            toggle_hotkey,
        };
        match crate::platform::validate_hotkeys(&candidate) {
            Ok(_) => {
                let summary = format!(
                    "新的录音热键：\n按住录音：{}\n切换录音：{}\n\n是否确认使用？",
                    display_binding(&candidate.hold_hotkey),
                    display_binding(&candidate.toggle_hotkey)
                );
                if ui.confirm(&summary, true) {
                    document.input = candidate;
                    return Ok(());
                }
            }
            Err(issues) => {
                let details = issues
                    .iter()
                    .map(|issue| format!("{}: {}", issue.key.as_str(), issue.message))
                    .collect::<Vec<_>>()
                    .join("\n");
                ui.message(&format!("热键设置无效：\n{}", safe_dialog_text(&details)));
            }
        }
    }
}

fn capture_binding(ui: &mut dyn SetupUi, mode: &str, current: &str) -> Result<String, ()> {
    let current_label = display_binding(current);
    loop {
        let prompt =
            format!("{mode}当前热键：{current_label}\n\n请按下一个键，然后点击“确定”继续。");
        match ui.capture_hotkey(&prompt) {
            Ok(Some(key)) => return Ok(key),
            Ok(None) => ui.message("未检测到按键。"),
            Err(error) => ui.message(&format!("无法记录热键：\n{}", safe_dialog_text(&error))),
        }
        if !ui.confirm("是否重试记录这个热键？", true) {
            return Err(());
        }
    }
}

fn display_binding(value: &str) -> String {
    if value.is_empty() {
        "未启用".to_string()
    } else {
        safe_dialog_text(value)
    }
}

fn choose_input_device(
    ui: &mut dyn SetupUi,
    current: Option<&str>,
    device_names: Result<Vec<String>, String>,
) -> Result<Option<String>, ()> {
    let names = match device_names {
        Ok(names) => names,
        Err(error) => {
            ui.message(&format!(
                "无法列出麦克风，将使用系统默认设备：\n{}",
                safe_dialog_text(&error)
            ));
            return Ok(None);
        }
    };
    let mut prompt = String::from("请选择麦克风编号：\n0 = 系统默认");
    for (index, name) in names.iter().enumerate() {
        prompt.push_str(&format!("\n{} = {}", index + 1, safe_dialog_text(name)));
    }
    if names
        .iter()
        .enumerate()
        .any(|(index, name)| names[..index].contains(name))
    {
        prompt.push_str("\n\n注意：检测到重名设备，录音时将使用第一个精确匹配项。");
    }
    let default = current
        .and_then(|current| names.iter().position(|name| name == current))
        .map_or_else(|| "0".to_string(), |index| (index + 1).to_string());
    loop {
        let Some(choice) = ui.input(&prompt, &default) else {
            return Err(());
        };
        match selected_input_device(choice.trim(), &names) {
            Ok(device) => return Ok(device),
            Err(error) => ui.message(&error),
        }
    }
}

fn selected_input_device(choice: &str, names: &[String]) -> Result<Option<String>, String> {
    let index = choice
        .parse::<usize>()
        .map_err(|_| "麦克风编号无效".to_string())?;
    if index == 0 {
        return Ok(None);
    }
    names
        .get(index - 1)
        .cloned()
        .map(Some)
        .ok_or_else(|| "麦克风编号超出范围".to_string())
}

fn safe_dialog_text(value: &str) -> String {
    value
        .chars()
        .take(2_000)
        .map(|character| match character {
            '\n' | '\r' | '\0' => ' ',
            '\'' => '’',
            '"' => '”',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::core::config::ConfigDocument;

    #[test]
    fn hotkey_setup_keeps_the_displayed_defaults() {
        let mut document = ConfigDocument::default();
        let mut ui = ScriptedUi {
            confirms: VecDeque::from([true]),
            inputs: VecDeque::new(),
            passwords: VecDeque::new(),
            captures: VecDeque::new(),
            messages: Vec::new(),
            confirm_messages: Vec::new(),
        };

        configure_hotkeys(&mut document, &mut ui).unwrap();

        assert_eq!(document.input.hold_hotkey, "F8");
        assert_eq!(document.input.toggle_hotkey, "F9");
    }

    #[test]
    fn hotkey_setup_captures_custom_keys_sequentially() {
        let mut document = ConfigDocument::default();
        let mut ui = ScriptedUi {
            confirms: VecDeque::from([false, true]),
            inputs: VecDeque::new(),
            passwords: VecDeque::new(),
            captures: VecDeque::from([Ok(Some("F10".to_string())), Ok(Some("F11".to_string()))]),
            messages: Vec::new(),
            confirm_messages: Vec::new(),
        };

        configure_hotkeys(&mut document, &mut ui).unwrap();

        assert_eq!(document.input.hold_hotkey, "F10");
        assert_eq!(document.input.toggle_hotkey, "F11");
    }

    #[test]
    fn hotkey_setup_only_applies_the_confirmed_pair() {
        let mut document = ConfigDocument::default();
        let mut ui = ScriptedUi {
            confirms: VecDeque::from([false, false, true]),
            inputs: VecDeque::new(),
            passwords: VecDeque::new(),
            captures: VecDeque::from([
                Ok(Some("F10".to_string())),
                Ok(Some("F11".to_string())),
                Ok(Some("F6".to_string())),
                Ok(Some("F7".to_string())),
            ]),
            messages: Vec::new(),
            confirm_messages: Vec::new(),
        };

        configure_hotkeys(&mut document, &mut ui).unwrap();

        assert_eq!(document.input.hold_hotkey, "F6");
        assert_eq!(document.input.toggle_hotkey, "F7");
    }

    #[test]
    fn input_device_choice_uses_default_or_exact_numbered_name() {
        let names = vec!["Built-in Mic".to_string(), "USB Mic".to_string()];
        assert_eq!(selected_input_device("0", &names).unwrap(), None);
        assert_eq!(
            selected_input_device("2", &names).unwrap(),
            Some("USB Mic".to_string())
        );
        assert!(selected_input_device("3", &names).is_err());
    }

    #[test]
    fn dynamic_dialog_text_cannot_inject_quotes_or_control_characters() {
        assert_eq!(
            safe_dialog_text("bad 'value'\n\0\"quoted\""),
            "bad ’value’  ”quoted”"
        );
    }

    struct ScriptedUi {
        confirms: VecDeque<bool>,
        inputs: VecDeque<Option<String>>,
        passwords: VecDeque<Option<String>>,
        captures: VecDeque<Result<Option<String>, String>>,
        messages: Vec<String>,
        confirm_messages: Vec<String>,
    }

    impl SetupUi for ScriptedUi {
        fn confirm(&mut self, message: &str, _default_yes: bool) -> bool {
            self.confirm_messages.push(message.to_string());
            self.confirms.pop_front().unwrap()
        }

        fn input(&mut self, _message: &str, _default: &str) -> Option<String> {
            self.inputs.pop_front().unwrap()
        }

        fn password(&mut self, _message: &str) -> Option<String> {
            self.passwords.pop_front().unwrap()
        }

        fn capture_hotkey(&mut self, _message: &str) -> Result<Option<String>, String> {
            self.captures.pop_front().unwrap()
        }

        fn message(&mut self, message: &str) {
            self.messages.push(message.to_string());
        }
    }

    struct UnexpectedVerifier;

    impl SetupVerifier for UnexpectedVerifier {
        fn verify(
            &mut self,
            _document: &ConfigDocument,
            _ui: &mut dyn SetupUi,
        ) -> Result<VerificationResult, String> {
            panic!("verification should have been skipped")
        }
    }

    struct SuccessfulVerifier;

    impl SetupVerifier for SuccessfulVerifier {
        fn verify(
            &mut self,
            _document: &ConfigDocument,
            _ui: &mut dyn SetupUi,
        ) -> Result<VerificationResult, String> {
            Ok(VerificationResult {
                raw: "测试".to_string(),
                final_text: "测试".to_string(),
            })
        }
    }

    #[test]
    fn verification_prompt_explains_the_window_transition() {
        let temp = tempfile::tempdir().unwrap();
        let store = ConfigStore::at(temp.path().join("config.json"));
        let mut ui = ScriptedUi {
            confirms: VecDeque::from([false, true, true, true]),
            inputs: VecDeque::from([
                Some("https://example.com/v1/audio/transcriptions".to_string()),
                Some("whisper-test".to_string()),
                Some("0".to_string()),
            ]),
            passwords: VecDeque::from([Some("disk-secret".to_string())]),
            captures: VecDeque::new(),
            messages: Vec::new(),
            confirm_messages: Vec::new(),
        };

        run_wizard(
            &store,
            ConfigDocument::default(),
            Ok(Vec::new()),
            &mut ui,
            &mut SuccessfulVerifier,
        )
        .unwrap();

        assert!(ui.confirm_messages.iter().any(|message| {
            message.contains("选择“是”后，此窗口会关闭") && message.contains("识别完成后会显示结果")
        }));
    }

    #[test]
    fn confirmed_wizard_saves_the_selected_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        let store = ConfigStore::at(path.clone());
        let mut ui = ScriptedUi {
            confirms: VecDeque::from([false, true, false, true]),
            inputs: VecDeque::from([
                Some("https://example.com/v1/audio/transcriptions".to_string()),
                Some("whisper-test".to_string()),
                Some("2".to_string()),
            ]),
            passwords: VecDeque::from([Some("disk-secret".to_string())]),
            captures: VecDeque::new(),
            messages: Vec::new(),
            confirm_messages: Vec::new(),
        };

        let outcome = run_wizard(
            &store,
            ConfigDocument::default(),
            Ok(vec!["Built-in Mic".to_string(), "USB Mic".to_string()]),
            &mut ui,
            &mut UnexpectedVerifier,
        )
        .unwrap();

        let WizardOutcome::Saved(document) = outcome else {
            panic!("wizard should save")
        };
        assert_eq!(document.audio.input_device.as_deref(), Some("USB Mic"));
        assert_eq!(document.input.hold_hotkey, "F8");
        assert_eq!(document.input.toggle_hotkey, "F9");
        assert_eq!(store.load().unwrap(), Some(*document));
        assert!(path.exists());
    }

    #[test]
    fn cancelling_before_confirmation_does_not_create_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        let store = ConfigStore::at(path.clone());
        let mut ui = ScriptedUi {
            confirms: VecDeque::new(),
            inputs: VecDeque::from([None]),
            passwords: VecDeque::new(),
            captures: VecDeque::new(),
            messages: Vec::new(),
            confirm_messages: Vec::new(),
        };

        let outcome = run_wizard(
            &store,
            ConfigDocument::default(),
            Ok(Vec::new()),
            &mut ui,
            &mut UnexpectedVerifier,
        )
        .unwrap();

        assert!(matches!(outcome, WizardOutcome::Cancelled));
        assert!(!path.exists());
    }
}
