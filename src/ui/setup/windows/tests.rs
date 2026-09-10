use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Threading::GetCurrentThreadId;

use super::*;
use crate::ui::setup::{NativeSetupUi, SetupUi, TITLE};

const CHILD_ENV: &str = "VIBERWHISPER_NATIVE_INPUT_TEST_CHILD";
const TEST_NAME: &str = "ui::setup::windows::tests::native_input_dialogs_show_and_return";
const PROMPT: &str = "请输入中文、引号 \" 和密码 🦀";

#[test]
fn native_input_dialogs_show_and_return() {
    // The old script adapter spins forever if its Chinese input window never appears. A child
    // process bounds this regression test even if either the UI or its driver stops responding.
    if std::env::var_os(CHILD_ENV).is_some() {
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
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "native input test failed: {status}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("native input did not appear or finish within 30 seconds");
        }
        thread::sleep(Duration::from_millis(20));
    }
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
