//! In-process Windows setup input. Native controls avoid tinyfiledialogs' script-host dependency
//! and its unbounded polling for a script window that may never be created.

use std::io;
use std::ptr::null_mut;

use anyhow::{Context, Result, anyhow, ensure};
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, SetLastError, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{EM_SETLIMITTEXT, EM_SETSEL};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const PROMPT_ID: u16 = 1000;
const INPUT_ID: u16 = 1001;

struct DialogState {
    title: Vec<u16>,
    prompt: Vec<u16>,
    initial: Vec<u16>,
    outcome: Option<Result<Option<String>>>,
}

/// Shows a modal input before the desktop event loop starts. `Some(default)` creates a text
/// input; `None` creates an empty masked password input. Cancellation is distinct from failure.
pub(super) fn input_box(
    title: &str,
    message: &str,
    default: Option<&str>,
) -> Result<Option<String>> {
    let mut state = DialogState {
        title: wide(title)?,
        prompt: wide(&message.replace("\r\n", "\n").replace('\n', "\r\n"))?,
        initial: wide(default.unwrap_or(""))?,
        outcome: None,
    };
    let template = dialog_template(default.is_none());
    // SAFETY: The template has DWORD alignment and Unicode strings. Both it and `state` remain
    // live until the synchronous modal call destroys the dialog and stops invoking its callback.
    let result = unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        if module.is_null() {
            return Err(io::Error::last_os_error()).context("failed to locate setup dialog module");
        }
        DialogBoxIndirectParamW(
            module,
            template.as_ptr().cast(),
            null_mut(),
            Some(dialog_proc),
            (&mut state as *mut DialogState) as LPARAM,
        )
    };
    if result == -1 || result == 0 {
        return Err(io::Error::last_os_error()).context("failed to create Windows setup input");
    }
    state
        .outcome
        .context("Windows setup input closed without a result")?
}

unsafe extern "system" fn dialog_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    // SAFETY: Windows invokes this callback for our own dialog. Its user data is initialized from
    // the live DialogState passed to the modal call. No Rust reference to mutable state is held
    // across a window API call, which may re-enter this callback with control notifications.
    unsafe {
        match message {
            WM_INITDIALOG => {
                let state = lparam as *mut DialogState;
                if let Err(error) = initialize_dialog(window, state) {
                    (*state).outcome = Some(Err(error));
                    EndDialog(window, IDABORT as isize);
                }
                0 // initialize_dialog sets input focus explicitly.
            }
            WM_COMMAND if (wparam & 0xffff) == IDOK as usize => {
                finish_dialog(
                    window,
                    read_text(GetDlgItem(window, INPUT_ID as i32)).map(Some),
                );
                1
            }
            WM_COMMAND if (wparam & 0xffff) == IDCANCEL as usize => {
                finish_dialog(window, Ok(None));
                1
            }
            WM_CLOSE => {
                finish_dialog(window, Ok(None));
                1
            }
            _ => 0,
        }
    }
}

unsafe fn initialize_dialog(window: HWND, state: *mut DialogState) -> Result<()> {
    // SAFETY: The caller provides its live dialog and state, whose buffers outlive every call.
    unsafe {
        SetLastError(0);
        if SetWindowLongPtrW(window, GWLP_USERDATA, state as isize) == 0 && GetLastError() != 0 {
            return Err(io::Error::last_os_error())
                .context("failed to initialize setup dialog state");
        }
        set_text(window, &(*state).title)?;
        set_text(GetDlgItem(window, PROMPT_ID as i32), &(*state).prompt)?;
        let input = GetDlgItem(window, INPUT_ID as i32);
        // Use the native control's maximum instead of its small default typing limit for API keys.
        SendMessageW(input, EM_SETLIMITTEXT, 0, 0);
        set_text(input, &(*state).initial)?;
        SendMessageW(input, EM_SETSEL, 0, -1);
        SetFocus(input);
    }
    Ok(())
}

unsafe fn finish_dialog(window: HWND, outcome: Result<Option<String>>) {
    // SAFETY: GWLP_USERDATA belongs to this dialog and is valid until the modal call returns.
    unsafe {
        let state = GetWindowLongPtrW(window, GWLP_USERDATA) as *mut DialogState;
        if !state.is_null() {
            (*state).outcome = Some(outcome);
            EndDialog(window, IDOK as isize);
        }
    }
}

fn wide(text: &str) -> Result<Vec<u16>> {
    ensure!(
        !text.contains('\0'),
        "setup dialog text contains a NUL character"
    );
    Ok(text.encode_utf16().chain(std::iter::once(0)).collect())
}

fn set_text(window: HWND, text: &[u16]) -> Result<()> {
    // SAFETY: Callers pass live controls and NUL-terminated buffers, read only during this call.
    if unsafe { SetWindowTextW(window, text.as_ptr()) } == 0 {
        return Err(io::Error::last_os_error()).context("failed to initialize setup input text");
    }
    Ok(())
}

fn read_text(window: HWND) -> Result<String> {
    // SAFETY: Callers provide a live window. The buffer includes room for the terminator and the
    // API receives its actual capacity. Clear last-error to distinguish empty text from failure.
    unsafe {
        ensure!(!window.is_null(), "Windows setup input control is missing");
        SetLastError(0);
        let length = GetWindowTextLengthW(window);
        if length == 0 && GetLastError() != 0 {
            return Err(io::Error::last_os_error()).context("failed to measure setup input text");
        }
        let capacity = length
            .checked_add(1)
            .context("setup input text is too long")?;
        let mut buffer = vec![0; capacity as usize];
        SetLastError(0);
        let copied = GetWindowTextW(window, buffer.as_mut_ptr(), capacity);
        if copied == 0 && GetLastError() != 0 {
            return Err(io::Error::last_os_error()).context("failed to read setup input text");
        }
        String::from_utf16(&buffer[..copied as usize])
            .map_err(|_| anyhow!("setup input is not valid Unicode"))
    }
}

fn dialog_template(password: bool) -> Vec<u32> {
    let style =
        WS_POPUP | WS_CAPTION | WS_SYSMENU | (DS_MODALFRAME | DS_SETFONT | DS_CENTER) as u32;
    // Standard DLGTEMPLATE: style, extended style, count, rectangle, menu, class, title, font.
    let mut words = vec![
        style as u16,
        (style >> 16) as u16,
        0,
        0,
        4,
        0,
        0,
        330,
        159,
        0,
        0,
        0,
        9,
    ];
    words.extend("Segoe UI".encode_utf16().chain(std::iter::once(0)));
    // A scrollable read-only prompt keeps long microphone lists accessible.
    add_control(
        &mut words,
        WS_CHILD | WS_VISIBLE | WS_VSCROLL | (ES_MULTILINE | ES_READONLY) as u32,
        [10, 10, 310, 85],
        PROMPT_ID,
        0x81,
        "",
    );
    let input_style = WS_CHILD
        | WS_VISIBLE
        | WS_BORDER
        | WS_TABSTOP
        | ES_AUTOHSCROLL as u32
        | if password { ES_PASSWORD as u32 } else { 0 };
    add_control(
        &mut words,
        input_style,
        [10, 104, 310, 14],
        INPUT_ID,
        0x81,
        "",
    );
    add_control(
        &mut words,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
        [207, 133, 54, 16],
        IDOK as u16,
        0x80,
        "确定",
    );
    add_control(
        &mut words,
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
        [267, 133, 54, 16],
        IDCANCEL as u16,
        0x80,
        "取消",
    );
    // Vec<u32> guarantees the DWORD base alignment required by DialogBoxIndirectParamW.
    words
        .chunks(2)
        .map(|pair| u32::from(pair[0]) | (u32::from(*pair.get(1).unwrap_or(&0)) << 16))
        .collect()
}

fn add_control(
    words: &mut Vec<u16>,
    style: u32,
    rectangle: [u16; 4],
    id: u16,
    class: u16,
    title: &str,
) {
    if !words.len().is_multiple_of(2) {
        words.push(0);
    }
    words.extend([style as u16, (style >> 16) as u16, 0, 0]);
    words.extend(rectangle);
    words.extend([id, 0xffff, class]);
    words.extend(title.encode_utf16().chain(std::iter::once(0)));
    words.push(0); // No control creation data.
}

#[cfg(test)]
mod tests;
