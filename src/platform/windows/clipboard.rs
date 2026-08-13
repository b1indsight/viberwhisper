use std::ffi::c_void;
use std::sync::OnceLock;

type Handle = *mut c_void;

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;
const HWND_MESSAGE: Handle = -3_isize as Handle;
const STATIC_CLASS: [u16; 7] = [83, 84, 65, 84, 73, 67, 0];

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: The guard exists only after this thread successfully opened the clipboard.
        unsafe { ffi::CloseClipboard() };
    }
}

fn owner_window() -> std::io::Result<Handle> {
    static OWNER: OnceLock<usize> = OnceLock::new();
    if let Some(owner) = OWNER.get() {
        return Ok(*owner as Handle);
    }

    // SAFETY: The system STATIC class is NUL-terminated; HWND_MESSAGE creates an invisible
    // process-lifetime window on the main thread.
    let owner = unsafe {
        ffi::CreateWindowExW(
            0,
            STATIC_CLASS.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            ffi::GetModuleHandleW(std::ptr::null()),
            std::ptr::null_mut(),
        )
    };
    if owner.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let _ = OWNER.set(owner as usize);
    Ok(owner)
}

pub(super) fn set_text(text: &str) -> std::io::Result<()> {
    if text.contains('\0') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "clipboard text contains an interior NUL",
        ));
    }

    let owner = owner_window()?;
    // SAFETY: `owner` is the live message-only window retained for the process lifetime.
    if unsafe { ffi::OpenClipboard(owner) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let _clipboard = ClipboardGuard;
    // SAFETY: This thread owns the open clipboard.
    if unsafe { ffi::EmptyClipboard() } == 0 {
        return Err(std::io::Error::last_os_error());
    }

    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    let byte_count = wide
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| std::io::Error::other("clipboard text is too large"))?;
    // SAFETY: The checked byte count covers the complete NUL-terminated UTF-16 buffer.
    let memory = unsafe { ffi::GlobalAlloc(GMEM_MOVEABLE, byte_count) };
    if memory.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `memory` is live and large enough for `wide`.
    let destination = unsafe { ffi::GlobalLock(memory) }.cast::<u16>();
    if destination.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe { ffi::GlobalFree(memory) };
        return Err(error);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(wide.as_ptr(), destination, wide.len());
        ffi::GlobalUnlock(memory);
    }

    // SAFETY: The clipboard is open and `memory` contains movable Unicode text. Windows owns
    // the handle after success; otherwise it remains ours to release.
    if unsafe { ffi::SetClipboardData(CF_UNICODETEXT, memory) }.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe { ffi::GlobalFree(memory) };
        return Err(error);
    }
    Ok(())
}

#[allow(non_snake_case)]
mod ffi {
    use super::Handle;

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn CreateWindowExW(
            dwExStyle: u32,
            lpClassName: *const u16,
            lpWindowName: *const u16,
            dwStyle: u32,
            X: i32,
            Y: i32,
            nWidth: i32,
            nHeight: i32,
            hWndParent: Handle,
            hMenu: Handle,
            hInstance: Handle,
            lpParam: Handle,
        ) -> Handle;
        pub fn OpenClipboard(hWndNewOwner: Handle) -> i32;
        pub fn CloseClipboard() -> i32;
        pub fn EmptyClipboard() -> i32;
        pub fn SetClipboardData(uFormat: u32, hMem: Handle) -> Handle;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetModuleHandleW(lpModuleName: *const u16) -> Handle;
        pub fn GlobalAlloc(uFlags: u32, dwBytes: usize) -> Handle;
        pub fn GlobalLock(hMem: Handle) -> Handle;
        pub fn GlobalUnlock(hMem: Handle) -> i32;
        pub fn GlobalFree(hMem: Handle) -> Handle;
    }
}
