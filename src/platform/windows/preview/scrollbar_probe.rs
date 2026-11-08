//! Native controls, screen captures and UIA checks for the production scrollbar renderer.

use super::*;
use windows_sys::Win32::UI::Controls::WM_MOUSELEAVE;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetCapture;

impl Scrollbar {
    pub(super) fn record(&self, window: HWND, message: u32, wparam: WPARAM, phase: &str) {
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
            "scrollbar trace: unix-ms={time:?}; {} {phase}; message={message:#06x}; wparam={wparam:#x}; capture={capture:p}; geometry={geometry:?}; pressed={}; prints={}; commits={}",
            if window == self.state().text.get() { "EDIT" } else { "parent" },
            self.pressed.get(),
            self.paints.get(),
            self.commits.get(),
        ));
    }
}

impl NativePreview {
    /// Run production rendering against undecorated controls on the hosted 96-DPI desktop.
    pub(crate) fn smoke_scrollbar(cleanup_fault: bool) -> Result<()> {
        let artifacts =
            std::env::var_os("VIBERWHISPER_PREVIEW_ARTIFACTS").map(std::path::PathBuf::from);
        if let Some(directory) = &artifacts {
            std::fs::create_dir_all(directory)?;
        }
        let mut preview = Self::new(Box::new(|_| {
            panic!("Scrollbar input submitted or cancelled text")
        }));
        preview.show(
            SessionId(902),
            "scrollbar fixture",
            "Scrollbar fixture",
            None,
            None,
        )?;
        ensure!(
            preview.scrollbar.is_some(),
            "Production preview did not automatically attach its scrollbar"
        );
        exercise(&mut preview, artifacts.as_deref(), cleanup_fault)
    }

    fn attach_scrollbar(&mut self) -> Result<()> {
        ensure!(self.scrollbar.is_none(), "Scrollbar is already attached");
        let windows = window_handles(&self.state)?;
        let styles = [GWL_STYLE, GWL_EXSTYLE]
            .map(|index| unsafe { GetWindowLongPtrW(self.state.text.get(), index) });
        // SAFETY: This fixture owns the same stable state/HWND lifetime as production.
        self.scrollbar = Some(unsafe { Scrollbar::new(&self.state)? });
        ensure!(
            window_handles(&self.state)? == windows,
            "Composition created a child or thread-top-level HWND"
        );
        ensure!(
            [GWL_STYLE, GWL_EXSTYLE]
                .map(|index| unsafe { GetWindowLongPtrW(self.state.text.get(), index) })
                == styles,
            "Composition changed native EDIT styles"
        );
        Ok(())
    }

    fn remove_scrollbar(&mut self) -> Result<()> {
        let scrollbar = self
            .scrollbar
            .take()
            .ok_or_else(|| anyhow!("No production scrollbar to remove"))?;
        let checked = scrollbar.check();
        drop(scrollbar);
        // GetWindowSubclass needs a v6 manifest; re-removal checks the same leaked observer
        // using the v5-compatible API already used by the production guard.
        for control in [self.state.window.get(), self.state.text.get()] {
            ensure!(
                unsafe { RemoveWindowSubclass(control, Some(observe), 71) } == 0,
                "Scrollbar observer outlived its guard"
            );
        }
        checked
    }
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
fn exercise(
    preview: &mut NativePreview,
    artifacts: Option<&std::path::Path>,
    cleanup_fault: bool,
) -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let directory = artifacts.unwrap_or_else(|| temporary.path());
    preview.update_text(&"第01行 hello 😀 中文\n".repeat(40), true, None)?;
    if cleanup_fault {
        // Deliberately exercise the real production renderer during native input cleanup.
        return desktop_driver(preview, directory, "input-cleanup");
    }
    let editor = preview.state.text.get();
    let current = unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) };
    unsafe { SendMessageW(editor, EM_LINESCROLL, 0, -current) };
    preview.remove_scrollbar()?;
    let baseline = directory.join("probe-scrollbar-native-baseline.png");
    preview.smoke_capture(&baseline)?;
    preview.attach_scrollbar()?;
    if preview.state.style.get().high_contrast {
        ensure!(
            preview.scrollbar.as_ref().unwrap().info().is_none(),
            "System colors enabled custom scrollbar drawing"
        );
        eprintln!(
            "scrollbar probe skipped: system-color fallback is active; no custom scrollbar validated"
        );
        anyhow::bail!("Desktop scrollbar gates require a non-high-contrast test desktop");
    }
    {
        let scrollbar = preview.scrollbar.as_ref().unwrap();
        let commits = scrollbar.commits.get();
        {
            let _drawing = scrollbar.composition.borrow_mut();
            // A nested repaint must not abort the process through RefCell::borrow_mut.
            unsafe { SendMessageW(editor, WM_NCPAINT, 1, 0) };
        }
        scrollbar.check()?;
        ensure!(
            scrollbar.commits.get() == commits,
            "Nested repaint committed during a draw"
        );
        scrollbar.sync();
        scrollbar.check()?;
    }
    for first in [0, 7, 25] {
        let scrollbar = preview.scrollbar.as_ref().unwrap();
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
        scrollbar.check()?;
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
            "Composition lost native scrollbar hit testing"
        );
        // Unlike sending WM_NCHITTEST, this asks Windows which real HWND is at the slot edge.
        ensure!(
            unsafe { WindowFromPoint(point) } == editor,
            "Native point hit testing did not select EDIT"
        );
    }
    let original = preview.state.style.replace(PreviewStyle::system());
    let native_fallback = {
        let scrollbar = preview.scrollbar.as_ref().unwrap();
        let paints = scrollbar.paints.get();
        scrollbar.sync();
        scrollbar.check()?;
        scrollbar.info().is_none()
            && scrollbar
                .composition
                .borrow()
                .as_ref()
                .is_some_and(|c| c.frame.is_none())
            && scrollbar.paints.get() == paints
            && unsafe { GetWindowLongPtrW(editor, GWL_STYLE) } & WS_VSCROLL as isize != 0
    };
    preview.state.style.set(original);
    ensure!(
        native_fallback,
        "System-color fallback lost the native scrollbar"
    );
    let window = preview.state.window.get();
    let guard = &**preview.scrollbar.as_ref().unwrap() as *const Scrollbar;
    preview.close();
    ensure!(
        preview
            .scrollbar
            .as_ref()
            .unwrap()
            .composition
            .borrow()
            .as_ref()
            .is_some_and(|c| c.frame.is_none()),
        "Closing a session retained its scrollbar frame"
    );
    for ready in [false, true] {
        if ready {
            preview.show(
                SessionId(903),
                "重新打开\n".repeat(40).as_str(),
                "Reopened",
                None,
                None,
            )?;
        } else {
            preview.show_streaming(
                SessionId(903),
                "重新打开\n".repeat(40).as_str(),
                "Reopened",
                None,
                true,
            )?;
        }
        let scrollbar = preview.scrollbar.as_ref().unwrap();
        scrollbar.check()?;
        ensure!(
            preview.state.window.get() == window
                && std::ptr::eq(&**scrollbar, guard)
                && scrollbar
                    .composition
                    .borrow()
                    .as_ref()
                    .is_some_and(|c| c.frame.is_some()),
            "Reopening replaced the HWND/renderer or lost its scrollable frame"
        );
    }
    preview.update_text("短句", true, None)?;
    {
        let scrollbar = preview.scrollbar.as_ref().unwrap();
        scrollbar.sync();
        scrollbar.check()?;
        ensure!(
            scrollbar.info().is_none()
                && scrollbar
                    .composition
                    .borrow()
                    .as_ref()
                    .is_some_and(|c| c.frame.is_none())
                && unsafe { GetWindowLongPtrW(editor, GWL_STYLE) } & WS_VSCROLL as isize == 0,
            "Short text kept its scrollbar drawing or native slot"
        );
    }
    let windows = window_handles(&preview.state)?;
    preview.remove_scrollbar()?;
    ensure!(
        window_handles(&preview.state)? == windows,
        "Removing composition changed native HWNDs"
    );
    for (ready, case) in [(true, "ready"), (false, "readonly")] {
        preview.update_text(&"第01行 hello 😀 中文\n".repeat(40), ready, None)?;
        desktop_driver(preview, directory, &format!("native-{case}"))?;
        let windows = window_handles(&preview.state)?;
        let styles =
            [GWL_STYLE, GWL_EXSTYLE].map(|index| unsafe { GetWindowLongPtrW(editor, index) });
        preview.attach_scrollbar()?;
        let result = desktop_driver(preview, directory, case);
        let closed = preview.remove_scrollbar(); // Flush traces and detach even if the driver failed.
        ensure!(
            window_handles(&preview.state)? == windows
                && [GWL_STYLE, GWL_EXSTYLE]
                    .map(|index| unsafe { GetWindowLongPtrW(editor, index) })
                    == styles,
            "Removing composition changed native HWNDs or EDIT styles"
        );
        result?;
        closed?;
        desktop_driver(preview, directory, &format!("restored-{case}"))?;
    }
    preview.attach_scrollbar()?;
    let scrollbar = preview.scrollbar.as_ref().unwrap();
    let windows = window_handles(&preview.state)?;
    let styles = [GWL_STYLE, GWL_EXSTYLE].map(|index| unsafe { GetWindowLongPtrW(editor, index) });
    scrollbar.fail(anyhow!("Injected runtime drawing error"));
    let commits = scrollbar.commits.get();
    scrollbar.sync();
    ensure!(
        scrollbar.check().is_err()
            && scrollbar.composition.borrow().is_none()
            && scrollbar.commits.get() == commits
            && window_handles(&preview.state)? == windows
            && [GWL_STYLE, GWL_EXSTYLE].map(|index| unsafe { GetWindowLongPtrW(editor, index) })
                == styles,
        "Runtime failure retained composition, retried or changed native windows/styles"
    );
    desktop_driver(preview, directory, "failed-readonly")?;
    eprintln!(
        "scrollbar production: reuse, print, actual screen, native input/drag, managed UIA and injected-error fallback checked at actual 96 DPI; no 4K claim"
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
