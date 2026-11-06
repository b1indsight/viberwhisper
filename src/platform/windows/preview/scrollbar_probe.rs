//! Test-only feasibility probe. Never enabled in the shipped preview before native validation.

use super::*;
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Graphics::Gdi::{OffsetViewportOrgEx, ScreenToClient, UpdateWindow};
use windows_sys::Win32::UI::Controls::STATE_SYSTEM_INVISIBLE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
};

/// A visual sibling only: all input and scroll geometry still belong to the native EDIT.
/// The probe guard must be dropped before its preview; it borrows the stable boxed state by HWND.
struct Overlay {
    state: *const WindowState,
    window: Cell<HWND>,
    pressed: Cell<bool>,
    old_clip_siblings: isize,
}

impl Overlay {
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

    fn sync(&self) {
        let state = self.state();
        if state.laying_out.get() {
            return;
        }
        let window = self.window.get();
        if let Some(info) = self.info() {
            let mut origin = POINT {
                x: info.rcScrollBar.left,
                y: info.rcScrollBar.top,
            };
            unsafe {
                ScreenToClient(state.window.get(), &mut origin);
                SetWindowPos(
                    window,
                    HWND_TOP,
                    origin.x,
                    origin.y,
                    info.rcScrollBar.right - info.rcScrollBar.left,
                    info.rcScrollBar.bottom - info.rcScrollBar.top,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
                InvalidateRect(window, null(), 0);
                UpdateWindow(window);
            }
        } else {
            unsafe { ShowWindow(window, SW_HIDE) };
        }
    }

    fn paint(&self, dc: HDC) {
        let Some(info) = self.info() else {
            return;
        };
        let state = self.state();
        let mut bounds = RECT::default();
        let mut cursor = POINT::default();
        unsafe {
            GetClientRect(self.window.get(), &mut bounds);
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
            SetDCBrushColor(dc, state.style.get().background);
            FillRect(dc, &bounds, GetStockObject(DC_BRUSH));
            SelectObject(dc, GetStockObject(DC_BRUSH));
            SelectObject(dc, GetStockObject(DC_PEN));
            SetDCBrushColor(dc, color);
            SetDCPenColor(dc, color);
            let width = state.pixels(6).min(bounds.right - 2).max(1);
            let left = (bounds.right - width) / 2;
            RoundRect(
                dc,
                left,
                info.xyThumbTop,
                left + width,
                info.xyThumbBottom,
                width,
                width,
            );
            RestoreDC(dc, saved);
        }
    }

    fn new(state: &WindowState) -> Result<Box<Self>> {
        static CLASS: OnceLock<Result<u16, String>> = OnceLock::new();
        let class_name = nul_terminated_utf16("ViberWhisperScrollbarProbe");
        let instance = unsafe { GetModuleHandleW(null()) };
        CLASS
            .get_or_init(|| {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(overlay_proc),
                    hInstance: instance,
                    lpszClassName: class_name.as_ptr(),
                    ..Default::default()
                };
                let atom = unsafe { RegisterClassW(&class) };
                if atom == 0 {
                    Err(std::io::Error::last_os_error().to_string())
                } else {
                    Ok(atom)
                }
            })
            .as_ref()
            .map_err(|error| anyhow!("Could not register scrollbar probe: {error}"))?;
        let old_style = unsafe { GetWindowLongPtrW(state.text.get(), GWL_STYLE) };
        let overlay = Box::new(Self {
            state,
            window: Cell::new(null_mut()),
            pressed: Cell::new(false),
            old_clip_siblings: old_style & WS_CLIPSIBLINGS as isize,
        });
        let reference = (&*overlay as *const Self) as usize;
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
                class_name.as_ptr(),
                null(),
                WS_CHILD,
                0,
                0,
                0,
                0,
                state.window.get(),
                null_mut(),
                instance,
                reference as *const _,
            )
        };
        ensure!(!window.is_null(), "Could not create scrollbar probe");
        overlay.window.set(window);
        unsafe {
            SetWindowLongPtrW(
                state.text.get(),
                GWL_STYLE,
                old_style | WS_CLIPSIBLINGS as isize,
            )
        };
        for control in [state.window.get(), state.text.get()] {
            ensure!(
                unsafe { SetWindowSubclass(control, Some(observe), 71, reference) } != 0,
                "Could not observe native scrollbar repaint"
            );
        }
        overlay.sync();
        Ok(overlay)
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        let state = self.state();
        unsafe {
            for control in [state.window.get(), state.text.get()] {
                RemoveWindowSubclass(control, Some(observe), 71);
            }
            if !self.window.get().is_null() {
                DestroyWindow(self.window.get());
            }
            let style = GetWindowLongPtrW(state.text.get(), GWL_STYLE);
            SetWindowLongPtrW(
                state.text.get(),
                GWL_STYLE,
                (style & !(WS_CLIPSIBLINGS as isize)) | self.old_clip_siblings,
            );
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
    let overlay = unsafe { &*(reference as *const Overlay) };
    if window == overlay.state().text.get() && message == WM_NCLBUTTONDOWN {
        overlay.pressed.set(true);
        overlay.sync();
    }
    if message == WM_NCMOUSEMOVE {
        let mut tracking = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE | TME_NONCLIENT,
            hwndTrack: window,
            dwHoverTime: 0,
        };
        unsafe { TrackMouseEvent(&mut tracking) };
    }
    let result = unsafe { DefSubclassProc(window, message, wparam, lparam) };
    if message == WM_NCLBUTTONDOWN {
        overlay.pressed.set(false);
    }
    if message == WM_PRINT && window == overlay.state().window.get() {
        // Compose at the same top z-order as the real sibling, regardless of default child order.
        if unsafe { IsWindowVisible(overlay.window.get()) } != 0 {
            let mut bounds = RECT::default();
            unsafe { GetWindowRect(overlay.window.get(), &mut bounds) };
            let mut origin = POINT {
                x: bounds.left,
                y: bounds.top,
            };
            let dc = wparam as HDC;
            unsafe {
                ScreenToClient(window, &mut origin);
                let saved = SaveDC(dc);
                OffsetViewportOrgEx(dc, origin.x, origin.y, null_mut());
                overlay.paint(dc);
                RestoreDC(dc, saved);
            }
        }
    } else if matches!(
        message,
        WM_PAINT
            | WM_NCPAINT
            | WM_COMMAND
            | WM_DPICHANGED
            | WM_SETTINGCHANGE
            | WM_SYSCOLORCHANGE
            | WM_THEMECHANGED
            | WM_VSCROLL
            | WM_MOUSEWHEEL
            | WM_KEYDOWN
            | WM_NCMOUSEMOVE
            | WM_NCMOUSELEAVE
            | WM_NCLBUTTONDOWN
            | EM_LINESCROLL
            | EM_SCROLLCARET
    ) {
        overlay.sync();
    }
    result
}

// SAFETY: Only the scoped probe creates this class, with its stable Box pointer as creation data.
unsafe extern "system" fn overlay_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize) };
    }
    let reference = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) };
    if reference != 0 {
        let overlay = unsafe { &*(reference as *const Overlay) };
        match message {
            WM_NCHITTEST => return HTTRANSPARENT as isize,
            WM_ERASEBKGND => return 1,
            WM_PRINTCLIENT => {
                overlay.paint(wparam as HDC);
                return 0;
            }
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let dc = unsafe { BeginPaint(window, &mut paint) };
                overlay.paint(dc);
                unsafe { EndPaint(window, &paint) };
                return 0;
            }
            _ => {}
        }
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Exercise both the print-only checks and separate real desktop input/UIA gates.
pub(super) fn exercise(
    preview: &mut NativePreview,
    artifacts: Option<&std::path::Path>,
) -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let directory = artifacts.unwrap_or_else(|| temporary.path());
    preview.update_text(&"第01行 hello 😀 中文\n".repeat(40), true, None)?;
    let editor = preview.state.text.get();
    let current = unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) };
    unsafe { SendMessageW(editor, EM_LINESCROLL, 0, -current) };
    let baseline = directory.join("probe-scrollbar-native-baseline.png");
    preview.smoke_capture(&baseline)?;
    let overlay = Overlay::new(&preview.state)?;
    if preview.state.style.get().high_contrast {
        ensure!(
            unsafe { IsWindowVisible(overlay.window.get()) } == 0,
            "System colors exposed overlay"
        );
        eprintln!(
            "scrollbar probe skipped: system-color fallback is active; no custom scrollbar validated"
        );
        anyhow::bail!("Desktop scrollbar gates require a non-high-contrast test desktop");
    }
    for first in [0, 7, 25] {
        let current = unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) };
        unsafe { SendMessageW(editor, EM_LINESCROLL, 0, first - current) };
        overlay.sync();
        ensure!(
            unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) } == first,
            "Probe changed native scroll position"
        );
        let info = overlay
            .info()
            .ok_or_else(|| anyhow!("Probe has no native scrollbar"))?;
        let mut bounds = RECT::default();
        ensure!(
            unsafe { GetWindowRect(overlay.window.get(), &mut bounds) } != 0
                && bounds.left == info.rcScrollBar.left
                && bounds.top == info.rcScrollBar.top
                && bounds.right == info.rcScrollBar.right
                && bounds.bottom == info.rcScrollBar.bottom,
            "Overlay does not match native scrollbar bounds"
        );
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
            unsafe { SendMessageW(overlay.window.get(), WM_NCHITTEST, 0, packed as isize) }
                == HTTRANSPARENT as isize,
            "Scrollbar probe intercepted hit testing"
        );
        // Unlike sending WM_NCHITTEST, this asks Windows which real HWND is at the slot edge.
        ensure!(
            unsafe { WindowFromPoint(point) } == editor,
            "Native point hit testing selected the visual overlay instead of EDIT"
        );
    }
    let original = preview.state.style.replace(PreviewStyle::system());
    overlay.sync();
    let native_fallback = unsafe { IsWindowVisible(overlay.window.get()) } == 0
        && unsafe { GetWindowLongPtrW(editor, GWL_STYLE) } & WS_VSCROLL as isize != 0;
    preview.state.style.set(original);
    ensure!(
        native_fallback,
        "System-color fallback lost the native scrollbar"
    );
    preview.update_text("短句", true, None)?;
    overlay.sync();
    ensure!(
        unsafe { IsWindowVisible(overlay.window.get()) } == 0,
        "Short text kept its scrollbar overlay"
    );
    drop(overlay);
    for (ready, case) in [(true, "ready"), (false, "readonly")] {
        preview.update_text(&"第01行 hello 😀 中文\n".repeat(40), ready, None)?;
        desktop_driver(preview, directory, &format!("native-{case}"))?;
        let overlay = Overlay::new(&preview.state)?;
        desktop_driver(preview, directory, case)?;
        drop(overlay);
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
/// requests. A watchdog releases input even if DispatchMessage is inside that modal loop.
fn desktop_driver(preview: &NativePreview, directory: &std::path::Path, case: &str) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, ExitStatus, Stdio};
    use std::sync::mpsc::channel;
    use std::thread;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
        MOUSEEVENTF_LEFTUP, MOUSEINPUT, SendInput,
    };

    let mut cursor = POINT::default();
    ensure!(
        unsafe { GetCursorPos(&mut cursor) } != 0,
        "Desktop input cannot save the pointer position"
    );
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
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()?;
    let editor = preview.state.text.get() as usize;
    let deadline = Instant::now() + Duration::from_secs(10);
    thread::scope(|scope| -> Result<()> {
        let (finished, watch) = channel::<bool>();
        let watchdog = scope.spawn(move || {
            if watch.recv_timeout(Duration::from_secs(10)) != Ok(true) {
                let release = [
                    INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 {
                            mi: MOUSEINPUT {
                                dwFlags: MOUSEEVENTF_LEFTUP,
                                ..Default::default()
                            },
                        },
                    },
                    INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VK_CONTROL,
                                dwFlags: KEYEVENTF_KEYUP,
                                ..Default::default()
                            },
                        },
                    },
                ];
                // SAFETY: The scoped preview outlives the watchdog. These are release-only
                // events for the fixture; cancellation lets a blocked native tracking loop exit.
                unsafe {
                    SendInput(
                        release.len() as u32,
                        release.as_ptr(),
                        size_of::<INPUT>() as i32,
                    );
                    PostMessageW(editor as HWND, WM_CANCELMODE, 0, 0);
                    SetCursorPos(cursor.x, cursor.y);
                }
            }
        });
        let result = (|| -> Result<ExitStatus> {
            loop {
                ensure!(
                    Instant::now() < deadline,
                    "Desktop scrollbar driver timed out: {case}"
                );
                if let Some(status) = child.try_wait()? {
                    return Ok(status);
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
        })();
        // Failure or disconnect also releases input; merely ending the watchdog early must not
        // leave a crashed driver's button pressed. A late result still fails the deadline above.
        let _ = finished.send(result.as_ref().is_ok_and(|status| status.success()));
        drop(finished);
        watchdog
            .join()
            .map_err(|_| anyhow!("Scrollbar input watchdog panicked"))?;
        if result.is_err() {
            // The process can exit between the timeout and kill; wait still collects its output.
            let _ = child.kill();
        }
        let output = child.wait_with_output()?;
        eprint!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        result?;
        ensure!(
            output.status.success(),
            "Desktop scrollbar driver failed ({case}): {}",
            output.status
        );
        Ok(())
    })
}
