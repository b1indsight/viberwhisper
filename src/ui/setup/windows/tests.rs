use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Threading::GetCurrentThreadId;

use super::*;
use crate::ui::setup::{NativeSetupUi, SetupUi, TITLE};

const CHILD_ENV: &str = "VIBERWHISPER_NATIVE_INPUT_TEST_CHILD";
const TEST_NAME: &str = "ui::setup::windows::tests::native_setup_dialogs_show_and_return";
const PROMPT: &str = "请输入中文、引号 \" 和密码 🦀";

#[test]
fn native_setup_dialogs_show_and_return() {
    // The old script adapter spins forever if its Chinese input window never appears. A child
    // process bounds this regression test even if either the UI or its driver stops responding.
    if std::env::var_os(CHILD_ENV).is_some() {
        exercise_message(Some(true));
        exercise_message(Some(false));
        exercise_message(None);
        for (default, entered, accept) in [
            (Some("默认值 \"中文\" 🦀"), "输入 \"中文\" 🦀", true),
            (None, "测试密钥", true),
            (None, "", true),
            (Some("existing"), "discard", false),
        ] {
            exercise_input(default, entered, accept);
        }
        return;
    }

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST_NAME, "--nocapture"])
        .env(CHILD_ENV, "1")
        // This environment used to select a console-only confirmation even in a GUI process.
        .env("SSH_CLIENT", "127.0.0.1 12345 22")
        .env_remove("DISPLAY")
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "native setup test failed: {}\n{}\n{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "native setup did not finish within 30 seconds\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn exercise_message(answer: Option<bool>) {
    eprintln!("opening native message: {answer:?}");
    // SAFETY: GetCurrentThreadId has no preconditions.
    let ui_thread = unsafe { GetCurrentThreadId() };
    let driver = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut dialog: HWND = std::ptr::null_mut();
            // SAFETY: The callback only writes to the live local HWND.
            unsafe {
                EnumThreadWindows(
                    ui_thread,
                    Some(find_message),
                    (&mut dialog as *mut HWND) as LPARAM,
                );
                if !dialog.is_null() {
                    eprintln!("native message visible; reading controls");
                    assert_eq!(read_text(dialog).unwrap(), TITLE);
                    assert_eq!(read_text(GetDlgItem(dialog, 0xffff)).unwrap(), PROMPT);
                    assert!(
                        GetWindow(dialog, GW_OWNER).is_null(),
                        "setup must not borrow another application's window"
                    );
                    let button = match answer {
                        Some(true) => IDYES,
                        Some(false) => IDNO,
                        None => IDOK,
                    };
                    eprintln!("completing native message with button {button}");
                    assert_ne!(
                        PostMessageW(
                            dialog,
                            WM_COMMAND,
                            button as WPARAM,
                            GetDlgItem(dialog, button) as LPARAM
                        ),
                        0
                    );
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "native confirmation or message did not appear"
            );
            thread::sleep(Duration::from_millis(10));
        }
    });
    let mut ui = NativeSetupUi;
    match answer {
        Some(answer) => assert_eq!(ui.confirm(PROMPT, answer).unwrap(), answer),
        None => ui.message(PROMPT).unwrap(),
    }
    eprintln!("native message returned: {answer:?}");
    driver.join().unwrap();
}

unsafe extern "system" fn find_message(window: HWND, data: LPARAM) -> i32 {
    // SAFETY: EnumThreadWindows supplies a live window; data points to a live local HWND.
    unsafe {
        if IsWindowVisible(window) != 0 && !GetDlgItem(window, 0xffff).is_null() {
            *(data as *mut HWND) = window;
            return 0;
        }
    }
    1
}

fn exercise_input(default: Option<&'static str>, entered: &'static str, accept: bool) {
    // SAFETY: GetCurrentThreadId has no preconditions.
    let ui_thread = unsafe { GetCurrentThreadId() };
    let driver = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let dialog = loop {
            let mut found: HWND = std::ptr::null_mut();
            // SAFETY: The callback only writes a HWND to the live local `found` pointer.
            unsafe {
                EnumThreadWindows(
                    ui_thread,
                    Some(find_input),
                    (&mut found as *mut HWND) as LPARAM,
                );
            }
            if !found.is_null() {
                break found;
            }
            assert!(
                Instant::now() < deadline,
                "native input window did not appear"
            );
            thread::sleep(Duration::from_millis(10));
        };
        // SAFETY: The modal dialog stays alive until this driver sends its completion command.
        unsafe {
            let input = GetDlgItem(dialog, INPUT_ID as i32);
            assert_eq!(read_text(dialog).unwrap(), TITLE);
            assert_eq!(
                read_text(GetDlgItem(dialog, PROMPT_ID as i32)).unwrap(),
                PROMPT
            );
            assert_eq!(read_text(input).unwrap(), default.unwrap_or(""));
            assert_eq!(
                GetWindowLongPtrW(input, GWL_STYLE) & ES_PASSWORD as isize != 0,
                default.is_none()
            );
            set_text(input, &wide(entered).unwrap()).unwrap();
            assert_ne!(
                PostMessageW(
                    dialog,
                    WM_COMMAND,
                    if accept { IDOK } else { IDCANCEL } as WPARAM,
                    0
                ),
                0
            );
        }
    });

    let mut ui = NativeSetupUi;
    let result = match default {
        Some(value) => ui.input(PROMPT, value),
        None => ui.password(PROMPT),
    };
    driver.join().unwrap();
    assert_eq!(result.unwrap().as_deref(), accept.then_some(entered));
}

unsafe extern "system" fn find_input(window: HWND, data: LPARAM) -> i32 {
    // SAFETY: EnumThreadWindows supplies a live window; data points to the driver's local HWND.
    unsafe {
        if IsWindowVisible(window) != 0 && !GetDlgItem(window, INPUT_ID as i32).is_null() {
            *(data as *mut HWND) = window;
            return 0;
        }
    }
    1
}
