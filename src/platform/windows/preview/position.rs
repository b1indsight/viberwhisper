//! Native caret geometry and monitor work areas in Win32 screen coordinates.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetClassNameW, GetGUIThreadInfo, GetWindowRect, SWP_NOACTIVATE, SWP_NOZORDER,
    SetWindowPos,
};

use crate::platform::preview::PreviewRect;

pub(super) fn capture_anchor(thread: u32, focus: HWND) -> Option<PreviewRect> {
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: Win32 receives correctly sized live structs and validates HWNDs.
    if unsafe { GetGUIThreadInfo(thread, &mut info) } != 0
        && info.hwndFocus == focus
        && !info.hwndCaret.is_null()
    {
        let mut point = POINT {
            x: info.rcCaret.left,
            y: info.rcCaret.top,
        };
        if unsafe { ClientToScreen(info.hwndCaret, &mut point) } != 0
            && let Some(rect) = PreviewRect::new(
                point.x as f64,
                point.y as f64,
                (info.rcCaret.right - info.rcCaret.left) as f64,
                (info.rcCaret.bottom - info.rcCaret.top) as f64,
            )
        {
            return Some(rect);
        }
    }
    // As with the parent's password check, limit control-specific assumptions
    // to native text controls; a browser renderer HWND is not an input rectangle.
    let mut class = [0u16; 64];
    let count = unsafe { GetClassNameW(focus, class.as_mut_ptr(), class.len() as i32) };
    let class = String::from_utf16_lossy(&class[..count.max(0) as usize]).to_ascii_lowercase();
    if class != "edit" && !class.starts_with("richedit") {
        return None;
    }
    let mut bounds = RECT::default();
    if unsafe { GetWindowRect(focus, &mut bounds) } == 0 {
        return None;
    }
    PreviewRect::new(
        bounds.left as f64,
        bounds.top as f64,
        (bounds.right - bounds.left) as f64,
        (bounds.bottom - bounds.top) as f64,
    )
}

pub(super) fn work_area(anchor: PreviewRect) -> Option<PreviewRect> {
    let point = POINT {
        x: (anchor.x + anchor.width / 2.0) as i32,
        y: (anchor.y + anchor.height / 2.0) as i32,
    };
    let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if monitor.is_null() || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    let work = info.rcWork;
    PreviewRect::new(
        work.left as f64,
        work.top as f64,
        (work.right - work.left) as f64,
        (work.bottom - work.top) as f64,
    )
}

pub(super) fn center(window: HWND) {
    let mut bounds = RECT::default();
    if unsafe { GetWindowRect(window, &mut bounds) } == 0 {
        return;
    }
    let Some(rect) = PreviewRect::new(
        bounds.left as f64,
        bounds.top as f64,
        (bounds.right - bounds.left) as f64,
        (bounds.bottom - bounds.top) as f64,
    ) else {
        return;
    };
    if let Some(work) = work_area(rect) {
        let moved = unsafe {
            SetWindowPos(
                window,
                std::ptr::null_mut(),
                (work.x + (work.width - rect.width) / 2.0).round() as i32,
                (work.y + (work.height - rect.height) / 2.0).round() as i32,
                0,
                0,
                SWP_NOZORDER
                    | SWP_NOACTIVATE
                    | windows_sys::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
            )
        };
        if moved == 0 {
            tracing::debug!("Could not center preview without a caret anchor");
        }
    }
}
