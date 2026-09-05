use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use rdev::listen;

use crate::core::config::InputSection;
use crate::input::hotkey::{EventMapper, HotkeyEvent, HotkeySource, parse_key};

const VERIFY_HELPER_ENV: &str = "VIBERWHISPER_VERIFY_HOTKEYS";
const VERIFY_HOLD_ENV: &str = "VIBERWHISPER_VERIFY_HOLD_HOTKEY";
const VERIFY_TOGGLE_ENV: &str = "VIBERWHISPER_VERIFY_TOGGLE_HOTKEY";
const START_PROTOCOL: &str = "start";
const STOP_PROTOCOL: &str = "stop";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerificationAction {
    Start,
    Stop,
}

#[derive(Debug, Default)]
struct VerificationState {
    active: Option<HotkeySource>,
}

impl VerificationState {
    fn handle(&mut self, event: HotkeyEvent) -> Option<VerificationAction> {
        match (self.active, event) {
            (None, HotkeyEvent::Pressed(source)) => {
                self.active = Some(source);
                Some(VerificationAction::Start)
            }
            (Some(HotkeySource::Hold), HotkeyEvent::Released(HotkeySource::Hold))
            | (Some(HotkeySource::Toggle), HotkeyEvent::Pressed(HotkeySource::Toggle)) => {
                Some(VerificationAction::Stop)
            }
            _ => None,
        }
    }
}

/// Runs the verification-only global hook inside the short-lived helper process.
pub(super) fn run_helper_if_requested() -> Result<bool, Box<dyn Error>> {
    if std::env::var_os(VERIFY_HELPER_ENV).is_none() {
        return Ok(false);
    }

    let hold_key = environment_key(VERIFY_HOLD_ENV)?;
    let toggle_key = environment_key(VERIFY_TOGGLE_ENV)?;
    if hold_key.is_none() && toggle_key.is_none() {
        return Err("verification requires at least one configured hotkey".into());
    }

    let state = Mutex::new((
        EventMapper::new(hold_key, toggle_key),
        VerificationState::default(),
    ));
    let result = listen(move |event| {
        let event_type = crate::platform::normalize_setup_hotkey_event(event.event_type);
        let action = {
            let mut state = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (mapper, verification) = &mut *state;
            mapper
                .map(&event_type)
                .and_then(|event| verification.handle(event))
        };
        let Some(action) = action else {
            return;
        };
        let protocol = match action {
            VerificationAction::Start => START_PROTOCOL,
            VerificationAction::Stop => STOP_PROTOCOL,
        };
        let mut stdout = std::io::stdout().lock();
        if writeln!(stdout, "{protocol}")
            .and_then(|()| stdout.flush())
            .is_err()
        {
            std::process::exit(2);
        }
        if action == VerificationAction::Stop {
            std::process::exit(0);
        }
    });

    match result {
        Ok(()) => Err("verification hotkey listener stopped unexpectedly".into()),
        Err(error) => {
            Err(format!("could not start verification hotkey listener: {error:?}").into())
        }
    }
}

fn environment_key(name: &str) -> Result<Option<rdev::Key>, Box<dyn Error>> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| format!("{name} is not valid text"))?;
    if value.is_empty() {
        return Ok(None);
    }
    parse_key(&value)
        .map(Some)
        .ok_or_else(|| format!("{name} contains an unsupported key").into())
}

/// Owns the helper process and exposes its single Start/Stop protocol to the verifier.
pub(super) struct VerificationListener {
    child: Child,
    actions: Receiver<Result<VerificationAction, String>>,
    finished: bool,
}

impl VerificationListener {
    pub(super) fn spawn(section: &InputSection) -> Result<Self, String> {
        let executable =
            std::env::current_exe().map_err(|error| format!("无法定位当前程序：{error}"))?;
        // Keep the hidden child-process launch behavior aligned with capture_hotkey_with_helper.
        let mut command = Command::new(executable);
        command
            .env(VERIFY_HELPER_ENV, "1")
            .env(VERIFY_HOLD_ENV, &section.hold_hotkey)
            .env(VERIFY_TOGGLE_ENV, &section.toggle_hotkey)
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
            .map_err(|error| format!("无法启动测试热键监听：{error}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "无法读取测试热键监听输出".to_string())?;
        let (action_tx, actions) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let action = line
                    .map_err(|error| format!("读取测试热键事件失败：{error}"))
                    .and_then(|line| match line.trim() {
                        START_PROTOCOL => Ok(VerificationAction::Start),
                        STOP_PROTOCOL => Ok(VerificationAction::Stop),
                        other => Err(format!("测试热键监听返回了未知事件：{other}")),
                    });
                if action_tx.send(action).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            actions,
            finished: false,
        })
    }

    pub(super) fn receive(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<VerificationAction>, String> {
        match self.actions.recv_timeout(timeout) {
            Ok(Ok(action)) => Ok(Some(action)),
            Ok(Err(error)) => Err(error),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => match self.finish() {
                Ok(()) => Err("测试热键监听在录音结束前退出".to_string()),
                Err(error) => Err(error),
            },
        }
    }

    pub(super) fn finish(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        let status = self
            .child
            .wait()
            .map_err(|error| format!("等待测试热键监听退出失败：{error}"))?;
        self.finished = true;
        if status.success() {
            return Ok(());
        }
        let mut detail = String::new();
        if let Some(mut stderr) = self.child.stderr.take() {
            let _ = stderr.read_to_string(&mut detail);
        }
        Err(format!("测试热键监听失败：{}", detail.trim()))
    }
}

impl Drop for VerificationListener {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::hotkey::{HotkeyEvent, HotkeySource};

    #[test]
    fn hold_press_and_release_bound_one_verification_recording() {
        let mut state = VerificationState::default();

        assert_eq!(
            state.handle(HotkeyEvent::Pressed(HotkeySource::Hold)),
            Some(VerificationAction::Start)
        );
        assert_eq!(state.handle(HotkeyEvent::Pressed(HotkeySource::Hold)), None);
        assert_eq!(
            state.handle(HotkeyEvent::Released(HotkeySource::Hold)),
            Some(VerificationAction::Stop)
        );
    }

    #[test]
    fn two_toggle_presses_bound_one_verification_recording() {
        let mut state = VerificationState::default();

        assert_eq!(
            state.handle(HotkeyEvent::Pressed(HotkeySource::Toggle)),
            Some(VerificationAction::Start)
        );
        assert_eq!(
            state.handle(HotkeyEvent::Released(HotkeySource::Toggle)),
            None
        );
        assert_eq!(
            state.handle(HotkeyEvent::Pressed(HotkeySource::Toggle)),
            Some(VerificationAction::Stop)
        );
    }
}
