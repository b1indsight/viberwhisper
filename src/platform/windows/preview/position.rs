//! Native caret geometry and monitor work areas in Win32 screen coordinates.

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, GetWindowDpiAwarenessContext, LogicalToPhysicalPointForPerMonitorDPI,
    SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetClassNameW, GetGUIThreadInfo, GetWindowRect, SWP_NOACTIVATE, SWP_NOZORDER,
    SetWindowPos,
};

use crate::platform::preview::PreviewRect;

/// A DPI override belongs to this thread and must be restored even on conversion failure.
struct ThreadDpiContext(DPI_AWARENESS_CONTEXT);

impl ThreadDpiContext {
    fn enter(context: DPI_AWARENESS_CONTEXT) -> Option<Self> {
        // SAFETY: The API validates the context and returns the previous thread context.
        let previous = unsafe { SetThreadDpiAwarenessContext(context) };
        if previous.is_null() {
            None
        } else {
            Some(Self(previous))
        }
    }
}

impl Drop for ThreadDpiContext {
    fn drop(&mut self) {
        // SAFETY: This guard retains a valid context returned on this same thread.
        if unsafe { SetThreadDpiAwarenessContext(self.0) }.is_null() {
            tracing::error!("Could not restore preview capture thread DPI context");
        }
    }
}

/// Transform both corners so DPI scaling applies to dimensions as well as position.
fn transform_bounds(
    bounds: RECT,
    mut transform: impl FnMut(POINT) -> Option<POINT>,
) -> Option<PreviewRect> {
    let top_left = transform(POINT {
        x: bounds.left,
        y: bounds.top,
    })?;
    let bottom_right = transform(POINT {
        x: bounds.right,
        y: bounds.bottom,
    })?;
    PreviewRect::new(
        top_left.x as f64,
        top_left.y as f64,
        bottom_right.x as f64 - top_left.x as f64,
        bottom_right.y as f64 - top_left.y as f64,
    )
}

/// Return screen geometry in physical pixels, independent of the target's DPI mode.
fn caret_bounds(window: HWND, bounds: RECT) -> Option<PreviewRect> {
    // rcCaret is in the target's logical client coordinates, not the caller's DPI mode.
    // Map to logical screen coordinates in that mode, then normalize both corners to pixels.
    let context = unsafe { GetWindowDpiAwarenessContext(window) };
    if context.is_null() {
        return None;
    }
    let _dpi = ThreadDpiContext::enter(context)?;
    transform_bounds(bounds, |mut point| {
        // SAFETY: The HWND is checked by Win32 and point is writable for both conversions.
        (unsafe { ClientToScreen(window, &mut point) } != 0
            && unsafe { LogicalToPhysicalPointForPerMonitorDPI(window, &mut point) } != 0)
            .then_some(point)
    })
}

pub(super) fn capture_anchor(thread: u32, focus: HWND) -> Option<PreviewRect> {
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: Win32 receives correctly sized live structs and validates HWNDs.
    if unsafe { GetGUIThreadInfo(thread, &mut info) } != 0
        && info.hwndFocus == focus
        && !info.hwndCaret.is_null()
        && let Some(rect) = caret_bounds(info.hwndCaret, info.rcCaret)
    {
        return Some(rect);
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::HiDpi::{
        AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        DPI_AWARENESS_CONTEXT_UNAWARE, GetThreadDpiAwarenessContext,
    };

    #[test]
    fn scaled_caret_converts_position_and_size_on_a_secondary_display() {
        // A 96-DPI editor on a 192-DPI display publishes logical client bounds.
        // Both corners must be converted; retaining the logical height overlaps text.
        let caret = RECT {
            left: 10,
            top: 20,
            right: 11,
            bottom: 40,
        };
        let bounds = transform_bounds(caret, |point| {
            Some(POINT {
                x: -1600 + point.x * 2,
                y: 100 + point.y * 2,
            })
        })
        .unwrap();
        assert_eq!(bounds, PreviewRect::new(-1580.0, 140.0, 2.0, 40.0).unwrap());
        let work = PreviewRect::new(-1920.0, 0.0, 1920.0, 1080.0).unwrap();
        assert_eq!(
            crate::platform::preview::preview_origin(bounds, (200.0, 52.0), work),
            (-1580.0, 186.0)
        );
    }

    #[test]
    fn failed_corner_conversion_restores_the_thread_dpi_context() {
        let original = unsafe { GetThreadDpiAwarenessContext() };
        let target = if unsafe {
            AreDpiAwarenessContextsEqual(original, DPI_AWARENESS_CONTEXT_UNAWARE)
        } != 0
        {
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
        } else {
            DPI_AWARENESS_CONTEXT_UNAWARE
        };
        let mut calls = 0;
        let result = {
            let _dpi = ThreadDpiContext::enter(target).expect("switch test thread DPI");
            assert_ne!(
                unsafe { AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), target) },
                0
            );
            transform_bounds(RECT::default(), |point| {
                calls += 1;
                (calls == 1).then_some(point)
            })
        };
        assert!(result.is_none());
        assert_eq!(calls, 2, "a failed second corner must reject the rectangle");
        assert_ne!(
            unsafe { AreDpiAwarenessContextsEqual(GetThreadDpiAwarenessContext(), original) },
            0
        );
    }
}
