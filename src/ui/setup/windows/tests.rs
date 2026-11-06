use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Graphics::Gdi::{GetObjectW, LOGFONTW};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE,
    GetAwarenessFromDpiAwarenessContext, GetDpiForWindow, GetThreadDpiAwarenessContext,
    GetWindowDpiAwarenessContext,
};

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
        crate::platform::initialize_dpi_awareness().unwrap();
        crate::platform::initialize_dpi_awareness().unwrap(); // An existing PMv2 context is valid.
        exercise_message(Some(true), false);
        exercise_message(Some(false), false);
        exercise_message(None, false);
        exercise_message(None, true);
        for (default, entered, accept) in [
            (Some("默认值 \"中文\" 🦀"), "输入 \"中文\" 🦀", true),
            (None, "测试密钥", true),
            (None, "", true),
            (Some("existing"), "discard", false),
        ] {
            exercise_input(default, entered, accept);
        }
        crate::platform::NativePreview::smoke_dpi().unwrap();
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
            print!("{}", String::from_utf8_lossy(&output.stdout));
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
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

fn exercise_message(answer: Option<bool>, close: bool) {
    eprintln!("opening native message: {answer:?}; close={close}");
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
                    let checks = std::panic::catch_unwind(|| {
                        assert_eq!(read_text(dialog).unwrap(), TITLE);
                        assert_eq!(
                            read_text(GetDlgItem(dialog, PROMPT_ID as i32)).unwrap(),
                            PROMPT
                        );
                        assert!(
                            GetWindow(dialog, GW_OWNER).is_null(),
                            "setup must not borrow another application's window"
                        );
                        assert_per_monitor_v2(dialog);
                        let default_button = match answer {
                            Some(true) => IDYES,
                            Some(false) => IDNO,
                            None => IDOK,
                        };
                        assert_eq!(
                            SendMessageW(dialog, DM_GETDEFID, 0, 0) & 0xffff,
                            default_button as isize,
                            "Enter must use the configured default answer"
                        );
                        if answer.is_some() {
                            // A yes/no question must not gain an implicit Cancel answer when
                            // replacing MessageBoxW. Both Alt+F4 and Esc leave it open.
                            SendMessageW(dialog, WM_CLOSE, 0, 0);
                            SendMessageW(dialog, WM_COMMAND, IDCANCEL as WPARAM, 0);
                            assert_ne!(IsWindow(dialog), 0);
                        }
                    });
                    // A failed check must still answer the modal dialog, otherwise the parent
                    // reports only its timeout instead of returning the actual assertion.
                    if let Some(answer) = answer {
                        let button = if answer { IDYES } else { IDNO };
                        eprintln!("completing native confirmation with button {button}");
                        assert_ne!(
                            PostMessageW(
                                dialog,
                                WM_COMMAND,
                                button as WPARAM,
                                GetDlgItem(dialog, button) as LPARAM
                            ),
                            0
                        );
                    } else if close {
                        eprintln!("closing native informational message");
                        assert_ne!(PostMessageW(dialog, WM_CLOSE, 0, 0), 0);
                    } else {
                        eprintln!("dismissing native informational message with Esc");
                        assert_ne!(PostMessageW(dialog, WM_COMMAND, IDCANCEL as WPARAM, 0), 0);
                    }
                    if let Err(panic) = checks {
                        std::panic::resume_unwind(panic);
                    }
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
    // SAFETY: Reading this thread's current context has no preconditions.
    unsafe {
        let context = GetThreadDpiAwarenessContext();
        eprintln!(
            "native message caller: thread={ui_thread}; context={context:p}; PMv2={}",
            AreDpiAwarenessContextsEqual(context, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
        );
    }
    match answer {
        Some(answer) => assert_eq!(ui.confirm(PROMPT, answer).unwrap(), answer),
        None => ui.message(PROMPT).unwrap(),
    }
    eprintln!("native message returned: {answer:?}");
    driver.join().unwrap();
}

fn assert_per_monitor_v2(window: HWND) {
    // SAFETY: The driver keeps this dialog open until after checking its native context.
    unsafe {
        let mut process = 0;
        let thread = GetWindowThreadProcessId(window, &raw mut process);
        let mut class = [0; 64];
        let length = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
        SetLastError(0);
        let context = GetWindowDpiAwarenessContext(window);
        let error = GetLastError();
        let dpi = GetDpiForWindow(window);
        eprintln!(
            "native setup HWND={window:p}; class={}; title={:?}; process={process}; thread={thread}; \
             context={context:p}; error={error}; awareness={}; PMv1={}; PMv2={}; system={}; DPI={dpi}",
            String::from_utf16_lossy(&class[..length as usize]),
            read_text(window),
            GetAwarenessFromDpiAwarenessContext(context),
            AreDpiAwarenessContextsEqual(context, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE),
            AreDpiAwarenessContextsEqual(context, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2),
            AreDpiAwarenessContextsEqual(context, DPI_AWARENESS_CONTEXT_SYSTEM_AWARE),
        );
        assert_ne!(
            AreDpiAwarenessContextsEqual(context, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,),
            0,
            "setup window must be PMv2 before winit starts"
        );
        assert!(dpi > 0);
    }
}

unsafe extern "system" fn find_message(window: HWND, data: LPARAM) -> i32 {
    // SAFETY: EnumThreadWindows supplies a live window; data points to a live local HWND.
    unsafe {
        if IsWindowVisible(window) != 0
            && !GetDlgItem(window, PROMPT_ID as i32).is_null()
            && GetDlgItem(window, INPUT_ID as i32).is_null()
        {
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
        let checks = std::panic::catch_unwind(|| unsafe {
            assert_per_monitor_v2(dialog);
            let input = GetDlgItem(dialog, INPUT_ID as i32);
            let dpi = GetDpiForWindow(dialog);
            assert_eq!(GetDpiForWindow(input), dpi);
            let font = SendMessageW(input, WM_GETFONT, 0, 0) as _;
            let mut description = LOGFONTW::default();
            assert_eq!(
                GetObjectW(
                    font,
                    size_of::<LOGFONTW>() as i32,
                    (&raw mut description).cast()
                ),
                size_of::<LOGFONTW>() as i32
            );
            // DS_SETFONT is 9pt; Windows scales DLU and the font once, not twice.
            assert!((description.lfHeight.abs() * 72 - 9 * dpi as i32).abs() <= 72);
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
        });
        // As with messages above, dismiss even on a failed check before resuming its panic.
        // SAFETY: The modal dialog is still alive; only this command completes it.
        unsafe {
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
        if let Err(panic) = checks {
            std::panic::resume_unwind(panic);
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
