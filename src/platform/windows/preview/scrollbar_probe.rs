//! Test-only feasibility probe. Never enabled in the shipped preview before native validation.

use super::*;
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::{
    DCX_CACHE, DCX_CLIPSIBLINGS, DCX_WINDOW, GetDCEx, IntersectClipRect, RDW_NOCHILDREN,
    RDW_UPDATENOW,
};
use windows_sys::Win32::UI::Controls::{STATE_SYSTEM_INVISIBLE, WM_MOUSELEAVE};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};

/// Paint only the existing EDIT's non-client slot; native input and geometry remain untouched.
/// The probe guard must be dropped before its preview; it borrows the stable boxed state by HWND.
struct Scrollbar {
    state: *const WindowState,
    pressed: Cell<bool>,
    hovered: Cell<bool>,
    paints: Cell<u64>,
    trace: RefCell<Vec<String>>,
}

impl Scrollbar {
    fn state(&self) -> &WindowState {
        // SAFETY: `exercise` keeps its preview alive until after this guard is dropped.
        unsafe { &*self.state }
    }

    fn info(&self) -> Option<SCROLLBARINFO> {
        let state = self.state();
        let mut info = SCROLLBARINFO {
            cbSize: size_of::<SCROLLBARINFO>() as u32,
            ..Default::default()
        };
        if !state.style.get().high_contrast
            && unsafe { GetWindowLongPtrW(state.text.get(), GWL_STYLE) } & WS_VSCROLL as isize != 0
            && unsafe { GetScrollBarInfo(state.text.get(), OBJID_VSCROLL, &mut info) } != 0
            && info.rgstate[0] & STATE_SYSTEM_INVISIBLE == 0
        {
            Some(info)
        } else {
            None
        }
    }

    fn redraw(&self) {
        // Invalidate on installation/visual-state changes only, never as a repair loop after
        // native scroll messages. WM_NCPAINT must itself suffice during native tracking.
        unsafe {
            RedrawWindow(
                self.state().text.get(),
                null(),
                null_mut(),
                RDW_FRAME | RDW_INVALIDATE | RDW_UPDATENOW | RDW_NOCHILDREN,
            );
        }
    }

    /// Both a window DC and WM_PRINT's supplied DC use EDIT-window coordinates here.
    fn paint(&self, dc: HDC) -> bool {
        let Some(info) = self.info() else {
            return false;
        };
        let state = self.state();
        let mut window = RECT::default();
        if dc.is_null() || unsafe { GetWindowRect(state.text.get(), &mut window) } == 0 {
            return false;
        }
        let bounds = RECT {
            left: info.rcScrollBar.left - window.left,
            top: info.rcScrollBar.top - window.top,
            right: info.rcScrollBar.right - window.left,
            bottom: info.rcScrollBar.bottom - window.top,
        };
        let mut cursor = POINT::default();
        unsafe {
            GetCursorPos(&mut cursor);
            let hover = cursor.x >= info.rcScrollBar.left
                && cursor.x < info.rcScrollBar.right
                && cursor.y >= info.rcScrollBar.top
                && cursor.y < info.rcScrollBar.bottom;
            let color = if self.pressed.get() {
                0x00c7c7c7
            } else if hover {
                0x00b0b0b0
            } else {
                0x00929292
            };
            let saved = SaveDC(dc);
            if saved == 0 {
                return false;
            }
            // The window DC includes the client area. Keep all writes in the native slot,
            // including the visually hidden arrows, and restore the caller's clip/objects.
            if IntersectClipRect(dc, bounds.left, bounds.top, bounds.right, bounds.bottom) == 0 {
                RestoreDC(dc, saved);
                return false;
            }
            SetDCBrushColor(dc, state.style.get().background);
            let filled = FillRect(dc, &bounds, GetStockObject(DC_BRUSH));
            SelectObject(dc, GetStockObject(DC_BRUSH));
            SelectObject(dc, GetStockObject(DC_PEN));
            SetDCBrushColor(dc, color);
            SetDCPenColor(dc, color);
            let width = state.pixels(6).min(bounds.right - bounds.left - 2).max(1);
            let left = bounds.left + (bounds.right - bounds.left - width) / 2;
            let drawn = RoundRect(
                dc,
                left,
                bounds.top + info.xyThumbTop,
                left + width,
                bounds.top + info.xyThumbBottom,
                width,
                width,
            );
            RestoreDC(dc, saved);
            if filled == 0 || drawn == 0 {
                return false;
            }
        }
        self.paints.set(self.paints.get() + 1);
        true
    }

    fn paint_frame(&self) -> bool {
        let editor = self.state().text.get();
        let dc = unsafe {
            GetDCEx(
                editor,
                null_mut(),
                DCX_WINDOW | DCX_CACHE | DCX_CLIPSIBLINGS,
            )
        };
        if dc.is_null() {
            return false;
        }
        let painted = self.paint(dc);
        unsafe { ReleaseDC(editor, dc) };
        painted
    }

    fn record(&self, window: HWND, message: u32, wparam: WPARAM, phase: &str) {
        if !matches!(
            message,
            WM_NCPAINT
                | WM_PAINT
                | WM_PRINT
                | WM_COMMAND
                | WM_VSCROLL
                | WM_MOUSEWHEEL
                | WM_NCLBUTTONDOWN
                | WM_NCLBUTTONDBLCLK
                | WM_NCLBUTTONUP
                | WM_LBUTTONDOWN
                | WM_LBUTTONUP
                | WM_NCMOUSEMOVE
                | WM_NCMOUSELEAVE
                | WM_MOUSEMOVE
                | WM_MOUSELEAVE
                | WM_CAPTURECHANGED
                | WM_CANCELMODE
                | WM_TIMER
                | WM_DPICHANGED
                | EM_LINESCROLL
                | EM_SCROLLCARET
        ) {
            return;
        }
        let geometry = self.info().map(|info| {
            (
                info.rcScrollBar.left,
                info.rcScrollBar.top,
                info.rcScrollBar.right,
                info.rcScrollBar.bottom,
                info.xyThumbTop,
                info.xyThumbBottom,
            )
        });
        let capture = unsafe { GetCapture() };
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_millis());
        // Query native state before borrowing: Win32 can synchronously re-enter the subclass.
        self.trace.borrow_mut().push(format!(
            "scrollbar trace: unix-ms={time:?}; {} {phase}; message={message:#06x}; wparam={wparam:#x}; capture={capture:p}; geometry={geometry:?}; pressed={}; paints={}",
            if window == self.state().text.get() { "EDIT" } else { "parent" },
            self.pressed.get(),
            self.paints.get(),
        ));
    }

    fn new(state: &WindowState) -> Result<Box<Self>> {
        let windows = window_handles(state)?;
        let styles = [GWL_STYLE, GWL_EXSTYLE]
            .map(|index| unsafe { GetWindowLongPtrW(state.text.get(), index) });
        let scrollbar = Box::new(Self {
            state,
            pressed: Cell::new(false),
            hovered: Cell::new(false),
            paints: Cell::new(0),
            trace: RefCell::new(Vec::new()),
        });
        let reference = (&*scrollbar as *const Self) as usize;
        for control in [state.window.get(), state.text.get()] {
            ensure!(
                unsafe { SetWindowSubclass(control, Some(observe), 71, reference) } != 0,
                "Could not observe native scrollbar repaint"
            );
        }
        scrollbar.redraw();
        ensure!(
            window_handles(state)? == windows,
            "Same-EDIT drawing created a child or thread-top-level HWND"
        );
        ensure!(
            [GWL_STYLE, GWL_EXSTYLE]
                .map(|index| unsafe { GetWindowLongPtrW(state.text.get(), index) })
                == styles,
            "Same-EDIT drawing changed native EDIT styles"
        );
        Ok(scrollbar)
    }
}

impl Drop for Scrollbar {
    fn drop(&mut self) {
        let state = self.state();
        unsafe {
            for control in [state.window.get(), state.text.get()] {
                RemoveWindowSubclass(control, Some(observe), 71);
            }
            RedrawWindow(
                state.text.get(),
                null(),
                null_mut(),
                RDW_FRAME | RDW_INVALIDATE | RDW_UPDATENOW | RDW_NOCHILDREN,
            );
        }
        for line in self.trace.get_mut() {
            eprintln!("{line}");
        }
    }
}

// SAFETY: The boxed probe outlives these same-thread callbacks and removes both subclasses on drop.
unsafe extern "system" fn observe(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass: usize,
    reference: usize,
) -> LRESULT {
    let scrollbar = unsafe { &*(reference as *const Scrollbar) };
    scrollbar.record(window, message, wparam, "enter");
    let editor = window == scrollbar.state().text.get();
    let tracking = editor
        && matches!(message, WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK)
        && wparam == HTVSCROLL as usize;
    if tracking {
        scrollbar.pressed.set(true);
        scrollbar.redraw();
    }
    if editor && message == WM_NCMOUSEMOVE {
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE | TME_NONCLIENT,
            hwndTrack: window,
            dwHoverTime: 0,
        };
        unsafe { TrackMouseEvent(&mut tracking) };
        if !scrollbar.hovered.replace(true) {
            scrollbar.redraw();
        }
    }
    if editor && message == WM_NCMOUSELEAVE && scrollbar.hovered.replace(false) {
        scrollbar.redraw();
    }
    let result = if editor && message == WM_NCPAINT && scrollbar.info().is_some() {
        // This EDIT has no border: its visible non-client area is the native vertical slot.
        // Do not call the native frame painter and then cover its pixels afterwards.
        if scrollbar.paint_frame() {
            0
        } else {
            unsafe { DefSubclassProc(window, message, wparam, lparam) }
        }
    } else if editor
        && message == WM_PRINT
        && lparam & PRF_NONCLIENT as isize != 0
        && scrollbar.info().is_some()
    {
        // Keep the owned-control print check without a sibling or parent-side composition.
        let result =
            unsafe { DefSubclassProc(window, message, wparam, lparam & !(PRF_NONCLIENT as isize)) };
        scrollbar.paint(wparam as HDC);
        result
    } else {
        unsafe { DefSubclassProc(window, message, wparam, lparam) }
    };
    if (tracking || (editor && message == WM_CAPTURECHANGED)) && scrollbar.pressed.replace(false) {
        scrollbar.redraw();
    }
    scrollbar.record(window, message, wparam, "exit");
    result
}

fn window_handles(state: &WindowState) -> Result<[Vec<usize>; 2]> {
    let mut handles = [Vec::new(), Vec::new()];
    // SAFETY: Each enumeration is synchronous and only appends live HWND values to its Vec.
    unsafe {
        EnumChildWindows(
            state.window.get(),
            Some(collect_window),
            (&mut handles[0] as *mut Vec<usize>) as isize,
        );
        let thread = GetWindowThreadProcessId(state.window.get(), null_mut());
        ensure!(
            EnumThreadWindows(
                thread,
                Some(collect_window),
                (&mut handles[1] as *mut Vec<usize>) as isize,
            ) != 0,
            "Could not enumerate native preview thread windows"
        );
    }
    for list in &mut handles {
        list.sort_unstable();
    }
    Ok(handles)
}

unsafe extern "system" fn collect_window(window: HWND, data: LPARAM) -> i32 {
    // SAFETY: Both synchronous enumerations pass a live, uniquely borrowed Vec<usize>.
    unsafe { &mut *(data as *mut Vec<usize>) }.push(window as usize);
    1
}

/// Exercise both the print-only checks and separate real desktop input/UIA gates.
pub(super) fn exercise(
    preview: &mut NativePreview,
    artifacts: Option<&std::path::Path>,
    cleanup_fault: bool,
) -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let directory = artifacts.unwrap_or_else(|| temporary.path());
    preview.update_text(&"第01行 hello 😀 中文\n".repeat(40), true, None)?;
    if cleanup_fault {
        return desktop_driver(preview, directory, "input-cleanup");
    }
    let editor = preview.state.text.get();
    let current = unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) };
    unsafe { SendMessageW(editor, EM_LINESCROLL, 0, -current) };
    let baseline = directory.join("probe-scrollbar-native-baseline.png");
    preview.smoke_capture(&baseline)?;
    let scrollbar = Scrollbar::new(&preview.state)?;
    if preview.state.style.get().high_contrast {
        ensure!(
            scrollbar.info().is_none(),
            "System colors enabled custom scrollbar drawing"
        );
        eprintln!(
            "scrollbar probe skipped: system-color fallback is active; no custom scrollbar validated"
        );
        anyhow::bail!("Desktop scrollbar gates require a non-high-contrast test desktop");
    }
    for first in [0, 7, 25] {
        let current = unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) };
        unsafe { SendMessageW(editor, EM_LINESCROLL, 0, first - current) };
        ensure!(
            unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) } == first,
            "Probe changed native scroll position"
        );
        let info = scrollbar
            .info()
            .ok_or_else(|| anyhow!("Probe has no native scrollbar"))?;
        let bounds = info.rcScrollBar;
        let capture = directory.join(format!("probe-scrollbar-first-{first}.png"));
        preview.smoke_capture(&capture)?;
        let mut window = RECT::default();
        ensure!(
            unsafe { GetWindowRect(preview.state.window.get(), &mut window) } != 0,
            "Could not measure scrollbar capture"
        );
        let (frame, pixels) = read_capture(&capture)?;
        let pixel = |x: i32, y: i32| {
            let offset = (((y - window.top) * frame.width as i32 + x - window.left) * 4) as usize;
            &pixels[offset..offset + 3]
        };
        let middle = bounds.top + (info.xyThumbTop + info.xyThumbBottom) / 2;
        ensure!(
            pixel(bounds.left + 1, middle) == [0x1e; 3]
                && [[0x92; 3], [0xb0; 3]]
                    .iter()
                    .any(|color| pixel((bounds.left + bounds.right) / 2, middle) == color),
            "WM_PRINT lost the dark rail or the narrow thumb"
        );
        let thumb_width = (bounds.left..bounds.right)
            .filter(|x| pixel(*x, middle) != [0x1e; 3])
            .count() as i32;
        ensure!(
            thumb_width
                == preview
                    .state
                    .pixels(6)
                    .min(bounds.right - bounds.left - 2)
                    .max(1),
            "Scrollbar probe did not retain a six-DIP thumb within the native slot"
        );
        if first == 0 {
            let (original, original_pixels) = read_capture(&baseline)?;
            ensure!(
                original.width == frame.width && original.height == frame.height,
                "Scrollbar probe changed preview size"
            );
            for (index, (before, after)) in original_pixels
                .as_chunks::<4>()
                .0
                .iter()
                .zip(pixels.as_chunks::<4>().0)
                .enumerate()
            {
                let x = window.left + (index as u32 % frame.width) as i32;
                let y = window.top + (index as u32 / frame.width) as i32;
                if x < bounds.left || x >= bounds.right || y < bounds.top || y >= bounds.bottom {
                    ensure!(
                        before == after,
                        "Scrollbar probe changed pixels outside the native slot"
                    );
                }
            }
        }
        let point = POINT {
            x: bounds.left + 1,
            y: bounds.top + (info.xyThumbTop + info.xyThumbBottom) / 2,
        };
        let packed = (point.x as u32 & 0xffff) | ((point.y as u32 & 0xffff) << 16);
        ensure!(
            unsafe { SendMessageW(editor, WM_NCHITTEST, 0, packed as isize) } == HTVSCROLL as isize,
            "Same-EDIT drawing lost native scrollbar hit testing"
        );
        // Unlike sending WM_NCHITTEST, this asks Windows which real HWND is at the slot edge.
        ensure!(
            unsafe { WindowFromPoint(point) } == editor,
            "Native point hit testing did not select EDIT"
        );
    }
    let original = preview.state.style.replace(PreviewStyle::system());
    let paints = scrollbar.paints.get();
    scrollbar.redraw();
    let native_fallback = scrollbar.info().is_none()
        && scrollbar.paints.get() == paints
        && unsafe { GetWindowLongPtrW(editor, GWL_STYLE) } & WS_VSCROLL as isize != 0;
    preview.state.style.set(original);
    ensure!(
        native_fallback,
        "System-color fallback lost the native scrollbar"
    );
    preview.update_text("短句", true, None)?;
    ensure!(
        scrollbar.info().is_none()
            && unsafe { GetWindowLongPtrW(editor, GWL_STYLE) } & WS_VSCROLL as isize == 0,
        "Short text kept its scrollbar drawing or native slot"
    );
    let windows = window_handles(&preview.state)?;
    drop(scrollbar);
    ensure!(
        window_handles(&preview.state)? == windows,
        "Removing same-EDIT drawing changed native HWNDs"
    );
    for (ready, case) in [(true, "ready"), (false, "readonly")] {
        preview.update_text(&"第01行 hello 😀 中文\n".repeat(40), ready, None)?;
        desktop_driver(preview, directory, &format!("native-{case}"))?;
        let windows = window_handles(&preview.state)?;
        let scrollbar = Scrollbar::new(&preview.state)?;
        let result = desktop_driver(preview, directory, case);
        drop(scrollbar); // Flush message traces on failures too, before returning the error.
        ensure!(
            window_handles(&preview.state)? == windows,
            "Removing same-EDIT drawing changed native HWNDs"
        );
        result?;
    }
    eprintln!(
        "scrollbar probe: print, actual screen, native input/drag and managed UIA gates checked at actual 96 DPI; NOT enabled in production"
    );
    Ok(())
}

fn read_capture(path: &std::path::Path) -> Result<(png::OutputInfo, Vec<u8>)> {
    let mut reader = png::Decoder::new(std::fs::File::open(path)?).read_info()?;
    let mut pixels = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut pixels)?;
    ensure!(
        frame.color_type == png::ColorType::Rgba && frame.bit_depth == png::BitDepth::Eight,
        "Scrollbar capture is not RGBA8"
    );
    pixels.truncate(frame.buffer_size());
    Ok((frame, pixels))
}

/// The separate client can drive the native modal tracking loop while this thread pumps UIA
/// requests. The non-GUI parent owns the sole deadline, process tree and input cleanup.
fn desktop_driver(preview: &NativePreview, directory: &std::path::Path, case: &str) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    let started = Instant::now();
    let system = std::env::var_os("WINDIR").ok_or_else(|| anyhow!("WINDIR is missing"))?;
    let mut child = Command::new(
        std::path::Path::new(&system).join("System32/WindowsPowerShell/v1.0/powershell.exe"),
    )
    .args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ])
    .arg(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/platform/windows/preview/scrollbar_probe.ps1"),
    )
    .arg("-Window")
    .arg((preview.state.window.get() as usize).to_string())
    .arg("-Editor")
    .arg((preview.state.text.get() as usize).to_string())
    .arg("-OwnerProcess")
    .arg(std::process::id().to_string())
    .arg("-Directory")
    .arg(directory)
    .arg("-Case")
    .arg(case)
    .arg("-ThumbWidth")
    .arg(preview.state.pixels(6).to_string())
    .creation_flags(0x08000000) // CREATE_NO_WINDOW
    .stdin(Stdio::null())
    // The parent drains these handles even if the GUI cannot return from DispatchMessage.
    .stdout(Stdio::inherit())
    .stderr(Stdio::inherit())
    .spawn()?;
    loop {
        if let Some(status) = child.try_wait()? {
            eprintln!(
                "scrollbar client {case} finished in {:?}: {status}",
                started.elapsed()
            );
            ensure!(
                status.success(),
                "Desktop scrollbar driver failed ({case}): {status}"
            );
            return Ok(());
        }
        let mut message = MSG::default();
        // SAFETY: This is the window-owning thread, with no active state/RefCell borrow.
        unsafe {
            while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
}
