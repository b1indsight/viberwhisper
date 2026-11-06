//! Native editing and delivery checks for the integration demo.

use super::super::nul_terminated_utf16;
use super::*;
use windows_sys::Win32::UI::Controls::{EM_CANUNDO, EM_GETSEL, EM_REPLACESEL};

#[cfg(test)]
#[path = "scrollbar_probe.rs"]
mod scrollbar_probe;

impl NativePreview {
    /// Ask only this fixture's HWND and native children to render into a PNG.
    pub(crate) fn smoke_capture(&self, path: &std::path::Path) -> Result<()> {
        use windows_sys::Win32::Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection,
            DIB_RGB_COLORS, DeleteDC, GdiFlush,
        };

        let window = self.state.window.get();
        let mut bounds = RECT::default();
        ensure!(
            unsafe { GetWindowRect(window, &mut bounds) } != 0,
            "Could not measure preview capture"
        );
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        ensure!(width > 0 && height > 0, "Preview capture has no area");
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // Top-down rows, matching the PNG order.
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let context = unsafe { CreateCompatibleDC(null_mut()) };
        ensure!(
            !context.is_null(),
            "Could not create preview capture context"
        );
        let mut pixels = null_mut();
        let bitmap =
            unsafe { CreateDIBSection(context, &info, DIB_RGB_COLORS, &mut pixels, null_mut(), 0) };
        if bitmap.is_null() {
            unsafe { DeleteDC(context) };
            anyhow::bail!("Could not create preview capture bitmap");
        }
        let previous = unsafe { SelectObject(context, bitmap) };
        let captured = (|| -> Result<Vec<u8>> {
            ensure!(
                !previous.is_null(),
                "Could not select preview capture bitmap"
            );
            // SAFETY: The selected 32-bit DIB owns width * height writable pixels until
            // cleanup below. WM_PRINT paints our own controls, never another application.
            unsafe {
                std::slice::from_raw_parts_mut(pixels.cast::<u8>(), (width * height * 4) as usize)
            }
            .fill(255);
            unsafe {
                SendMessageW(
                    window,
                    WM_PRINT,
                    context as usize,
                    (PRF_CLIENT | PRF_NONCLIENT | PRF_CHILDREN | PRF_ERASEBKGND) as isize,
                );
            }
            ensure!(
                unsafe { GdiFlush() } != 0,
                "Could not flush preview capture"
            );
            let mut rgba = unsafe {
                std::slice::from_raw_parts(pixels.cast::<u8>(), (width * height * 4) as usize)
            }
            .to_vec();
            for pixel in rgba.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2); // GDI BGRA to PNG RGBA; GDI does not set alpha.
                pixel[3] = 255;
            }
            Ok(rgba)
        })();
        unsafe {
            if !previous.is_null() {
                SelectObject(context, previous);
            }
            DeleteObject(bitmap);
            DeleteDC(context);
        }
        let rgba = captured?;
        let mut encoder =
            png::Encoder::new(std::fs::File::create(path)?, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&rgba)?;
        let pixel = |x: i32, y: i32| {
            let offset = ((y * width + x) * 4) as usize;
            u32::from(rgba[offset])
                | (u32::from(rgba[offset + 1]) << 8)
                | (u32::from(rgba[offset + 2]) << 16)
        };
        let (background, border) = if self.state.style.get().high_contrast {
            unsafe { (GetSysColor(COLOR_WINDOW), GetSysColor(COLOR_WINDOWTEXT)) }
        } else {
            (0x001e1e1e, 0x00343434)
        };
        ensure!(
            pixel(2, height / 2) == background && pixel(width / 2, 0) == border,
            "Preview capture lost its background or thin outline: {}",
            path.display()
        );
        eprintln!(
            "preview capture: {}; {width}x{height}; actual DPI={}; layout DPI={}; read-only={}; warning={}; lines={}; first-visible-line={}; vertical-scrollbar={}; system-colors={}",
            path.display(),
            unsafe { GetDpiForWindow(window) },
            self.state.dpi.get(),
            unsafe { GetWindowLongPtrW(self.state.text.get(), GWL_STYLE) } & ES_READONLY as isize
                != 0,
            self.state.message_visible.get(),
            unsafe { SendMessageW(self.state.text.get(), EM_GETLINECOUNT, 0, 0) },
            unsafe { SendMessageW(self.state.text.get(), EM_GETFIRSTVISIBLELINE, 0, 0) },
            unsafe { GetWindowLongPtrW(self.state.text.get(), GWL_STYLE) } & WS_VSCROLL as isize
                != 0,
            self.state.style.get().high_contrast,
        );
        Ok(())
    }

    /// Empty errors cannot submit text; exercise the same native key subclass as users.
    #[cfg(feature = "integration")]
    pub(crate) fn smoke_error(&self, verify_pending: &dyn Fn() -> Result<()>) -> Result<()> {
        ensure!(
            self.state.normalized_text()?.is_empty()
                && !self.state.retry_allowed.get()
                && unsafe { GetWindowLongPtrW(self.state.text.get(), GWL_STYLE) }
                    & ES_READONLY as isize
                    != 0,
            "Error preview retained text, accepted edits, or enabled confirmation"
        );
        let mut message =
            vec![0u16; unsafe { GetWindowTextLengthW(self.state.status.get()) } as usize + 1];
        let count = unsafe {
            GetWindowTextW(
                self.state.status.get(),
                message.as_mut_ptr(),
                message.len() as i32,
            )
        };
        ensure!(
            unsafe { IsWindowVisible(self.state.status.get()) } != 0
                && String::from_utf16_lossy(&message[..count as usize]).contains("Esc"),
            "Error preview did not explain cancellation"
        );
        for key in [VK_RETURN, VK_ESCAPE] {
            unsafe {
                SendMessageW(self.state.text.get(), WM_KEYDOWN, key as usize, 0);
                SendMessageW(self.state.text.get(), WM_KEYUP, key as usize, 0);
            }
            if key == VK_RETURN {
                verify_pending()?;
            }
        }
        Ok(())
    }

    /// Exercise real HWNDs with synthetic DPI notifications; this is not a high-DPI display test.
    #[cfg(test)]
    pub(crate) fn smoke_dpi() -> Result<()> {
        use windows_sys::Win32::Graphics::Gdi::{GetObjectW, LOGFONTW};
        use windows_sys::Win32::UI::HiDpi::{
            AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            GetWindowDpiAwarenessContext, SystemParametersInfoForDpi,
        };

        let artifacts =
            std::env::var_os("VIBERWHISPER_PREVIEW_ARTIFACTS").map(std::path::PathBuf::from);
        if let Some(directory) = &artifacts {
            std::fs::create_dir_all(directory)?;
        }

        unsafe extern "system" fn dpi_during_resize(
            window: HWND,
            message: u32,
            wparam: WPARAM,
            lparam: LPARAM,
            subclass: usize,
            reference: usize,
        ) -> LRESULT {
            if message == WM_WINDOWPOSCHANGED {
                // A monitor transition can notify DPI inside SetWindowPos. Remove the fixture
                // first so applying its suggested rectangle cannot inject another notification.
                unsafe {
                    RemoveWindowSubclass(window, Some(dpi_during_resize), subclass);
                    SendMessageW(window, WM_DPICHANGED, 144 | (144 << 16), reference as isize);
                }
            }
            unsafe { DefSubclassProc(window, message, wparam, lparam) }
        }

        let mut preview = Self::new(Box::new(|_| {
            panic!("DPI change submitted or cancelled text")
        }));
        preview.show_streaming(SessionId(901), "hello 😀", "DPI fixture", None, false)?;
        let window = preview.state.window.get();
        ensure!(
            unsafe {
                AreDpiAwarenessContextsEqual(
                    GetWindowDpiAwarenessContext(window),
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                )
            } != 0,
            "Preview is not PMv2"
        );
        let text = "hello 😀 中文 line\n".repeat(40);
        for (text, ready, warning, scrollable) in [
            ("hello 😀 中文", false, None, false),
            (text.as_str(), false, None, true),
            (text.as_str(), true, Some("测试提示"), true),
        ] {
            preview.update_text(text, ready, warning)?;
            let editor = preview.state.text.get();
            unsafe {
                if ready {
                    let suffix = nul_terminated_utf16("user edit");
                    SendMessageW(editor, EM_SETSEL, usize::MAX, -1);
                    SendMessageW(editor, EM_REPLACESEL, 1, suffix.as_ptr() as isize);
                }
                SendMessageW(editor, EM_SETSEL, 6, 8); // Select an entire surrogate pair.
                SendMessageW(editor, EM_LINESCROLL, 0, 5);
            }
            let original = preview.state.normalized_text()?;
            let first = unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) };
            ensure!(
                (first > 0) == scrollable,
                "DPI fixture did not keep the expected scroll state"
            );
            for (step, (dpi, reentrant)) in [
                (96, false),
                (144, false),
                (192, false),
                (96, false),
                (144, true),
            ]
            .into_iter()
            .enumerate()
            {
                let pixels = |logical: i32| (logical * dpi + 48) / 96;
                let suggested = RECT {
                    left: 41,
                    top: 67,
                    right: 1079,
                    bottom: 679,
                };
                // Dispatch through the real window procedure, not directly to the layout helper.
                if reentrant {
                    // Ensure the next layout really resizes; a no-op position need not notify.
                    ensure!(
                        unsafe {
                            SetWindowPos(
                                window,
                                null_mut(),
                                0,
                                0,
                                1,
                                1,
                                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                            )
                        } != 0,
                        "Could not prepare reentrant DPI resize"
                    );
                    // SAFETY: The RECT stays alive until the synchronous layout returns.
                    ensure!(
                        unsafe {
                            SetWindowSubclass(
                                window,
                                Some(dpi_during_resize),
                                2,
                                (&raw const suggested) as usize,
                            )
                        } != 0,
                        "Could not install reentrant DPI fixture"
                    );
                    let result = preview.state.layout();
                    unsafe { RemoveWindowSubclass(window, Some(dpi_during_resize), 2) };
                    result?;
                } else {
                    unsafe {
                        SendMessageW(
                            window,
                            WM_DPICHANGED,
                            dpi as usize | ((dpi as usize) << 16),
                            (&raw const suggested) as isize,
                        );
                    }
                }
                let mut bounds = RECT::default();
                let mut edit_bounds = RECT::default();
                ensure!(
                    unsafe { GetWindowRect(window, &mut bounds) } != 0
                        && unsafe { GetWindowRect(editor, &mut edit_bounds) } != 0,
                    "Could not read DPI-scaled bounds"
                );
                ensure!(
                    bounds.bottom - bounds.top
                        == pixels(if !scrollable {
                            52
                        } else if ready {
                            264
                        } else {
                            220
                        })
                        && bounds.right - bounds.left <= pixels(420)
                        && edit_bounds.left - bounds.left == pixels(16)
                        && edit_bounds.top - bounds.top == pixels(12)
                        && edit_bounds.bottom - edit_bounds.top
                            == pixels(if scrollable { 196 } else { 28 })
                        && bounds.top == suggested.top
                        && ((bounds.left + bounds.right) - (suggested.left + suggested.right))
                            .abs()
                            <= 1,
                    "Preview did not apply {dpi}-DPI layout and suggested placement"
                );
                ensure!(
                    (unsafe { GetWindowLongPtrW(editor, GWL_STYLE) } & WS_VSCROLL as isize != 0)
                        == scrollable
                        && (scrollable
                            || unsafe { SendMessageW(editor, EM_GETLINECOUNT, 0, 0) } == 1),
                    "DPI change wrapped short text or kept the wrong scrollbar state"
                );
                let mut expected = NONCLIENTMETRICSW {
                    cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
                    ..Default::default()
                };
                ensure!(
                    unsafe {
                        SystemParametersInfoForDpi(
                            SPI_GETNONCLIENTMETRICS,
                            expected.cbSize,
                            (&raw mut expected).cast(),
                            0,
                            dpi as u32,
                        )
                    } != 0,
                    "Could not read system font for DPI fixture"
                );
                for control in [editor, preview.state.status.get()] {
                    let font = unsafe { SendMessageW(control, WM_GETFONT, 0, 0) } as _;
                    let mut actual = LOGFONTW::default();
                    let expected_height = if control == editor {
                        (f64::from(expected.lfMessageFont.lfHeight) * 1.25).round() as i32
                    } else {
                        expected.lfMessageFont.lfHeight
                    };
                    ensure!(
                        unsafe {
                            GetObjectW(font, size_of::<LOGFONTW>() as i32, (&raw mut actual).cast())
                        } == size_of::<LOGFONTW>() as i32
                            && actual.lfHeight == expected_height
                            && actual.lfFaceName == expected.lfMessageFont.lfFaceName,
                        "Preview lost the body/status font proportions or system family at {dpi} DPI"
                    );
                }
                let mut start = 0u32;
                let mut end = 0u32;
                unsafe {
                    SendMessageW(
                        editor,
                        EM_GETSEL,
                        (&raw mut start) as usize,
                        (&raw mut end) as isize,
                    );
                }
                ensure!(
                    preview.state.normalized_text()? == original
                        && (start, end) == (6, 8)
                        && unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) } == first
                        && (unsafe { SendMessageW(editor, EM_CANUNDO, 0, 0) } != 0) == ready
                        && (unsafe { GetWindowLongPtrW(editor, GWL_STYLE) } & ES_READONLY as isize
                            != 0)
                            != ready,
                    "DPI change lost text, selection, scroll, undo, or read-only state"
                );
                eprintln!(
                    "native preview: synthetic {dpi} DPI; ready={ready}; scrollable={scrollable}; reentrant={reentrant}; font/layout/editor state checked"
                );
                if let Some(directory) = &artifacts {
                    let state = if !scrollable {
                        "short"
                    } else if ready {
                        "ready-warning"
                    } else {
                        "streaming"
                    };
                    preview.smoke_capture(
                        &directory.join(format!("synthetic-{dpi}-{state}-step-{step}.png")),
                    )?;
                }
            }
        }
        preview.smoke_streaming()?;
        // The demo calls smoke_streaming for each revision under one delivery deadline.
        // Keep the full scroll regression suite in this once-per-run native fixture.
        preview.smoke_scrolling(artifacts.as_deref())?;
        preview.smoke_appearance(artifacts.as_deref())?;
        scrollbar_probe::exercise(&mut preview, artifacts.as_deref())
    }

    #[cfg(test)]
    fn smoke_scrolling(&mut self, artifacts: Option<&std::path::Path>) -> Result<()> {
        let numbered: Vec<_> = (1..=41)
            .map(|line| format!("第{line:02}行：hello 😀 中文"))
            .collect();
        let scrolled = numbered[..40].join("\n");
        self.update_text(&scrolled, false, None)?;
        let editor = self.state.text.get();
        let mut viewport = RECT::default();
        unsafe { SendMessageW(editor, EM_GETRECT, 0, (&raw mut viewport) as isize) };
        let (_, line_height) = self.state.measure_text(editor, None)?;
        let fitting_lines = (viewport.bottom - viewport.top) / line_height;
        ensure!(
            fitting_lines > 5 && fitting_lines + 8 < 40,
            "Scroll fixture does not have enough lines for its native viewport"
        );
        for ready in [false, true] {
            // A Realtime retraction can remove the scrollbar from an already-scrolled
            // multiline draft. Keep all fitting text visible without stealing valid scroll.
            let tail = 40 - fitting_lines;
            for (case, lines, first, expected_first) in [
                ("shrink-fit", fitting_lines, 5, 0),
                ("shrink-clamp", fitting_lines + 3, 5, 3),
                ("shrink-preserve", fitting_lines + 8, 5, 5),
                ("append-tail", 41, tail, tail + 1),
                ("append-before-tail", 41, tail - 1, tail - 1),
            ] {
                self.update_text(&scrolled, ready, None)?;
                let editor = self.state.text.get();
                unsafe { SendMessageW(editor, EM_SETSEL, 11, 13) }; // A whole surrogate pair.
                let current = unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) };
                unsafe { SendMessageW(editor, EM_LINESCROLL, 0, first as isize - current) };
                ensure!(
                    unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) } == first as isize,
                    "Could not prepare scrolled preview for {case}"
                );
                let text = numbered[..lines as usize].join("\n");
                self.update_text(&text, ready, None)?;
                let actual_first = unsafe { SendMessageW(editor, EM_GETFIRSTVISIBLELINE, 0, 0) };
                eprintln!(
                    "native preview scroll: {case}; ready={ready}; lines={lines}; first={first}->{actual_first}; expected={expected_first}"
                );
                if let Some(directory) = artifacts {
                    self.smoke_capture(
                        &directory.join(format!("scroll-{case}-ready-{ready}.png")),
                    )?;
                }
                let mut start = 0u32;
                let mut end = 0u32;
                unsafe {
                    SendMessageW(
                        editor,
                        EM_GETSEL,
                        (&raw mut start) as usize,
                        (&raw mut end) as isize,
                    );
                }
                let style = unsafe { GetWindowLongPtrW(editor, GWL_STYLE) };
                ensure!(
                    self.state.normalized_text()? == text
                        && unsafe { SendMessageW(editor, EM_GETLINECOUNT, 0, 0) } == lines as isize
                        && actual_first == expected_first as isize
                        && (style & WS_VSCROLL as isize != 0) == (lines > fitting_lines)
                        && (style & ES_READONLY as isize != 0) != ready
                        && (start, end) == (11, 13)
                        && unsafe { SendMessageW(editor, EM_CANUNDO, 0, 0) } == 0,
                    "Scrolled preview lost viewport or editor state for {case}; ready={ready}"
                );
            }
        }
        Ok(())
    }

    /// Exercise actual control colors, wrapped warnings and a local system-color override.
    /// The override never changes the OS high-contrast setting or proves that OS transition.
    #[cfg(test)]
    fn smoke_appearance(&mut self, artifacts: Option<&std::path::Path>) -> Result<()> {
        use windows_sys::Win32::Graphics::Gdi::{GetBkColor, GetDCBrushColor, GetTextColor};

        let original = self.state.style.get();
        let result = (|| {
            for system_colors in [false, true] {
                if system_colors {
                    self.state.style.set(PreviewStyle::system());
                }
                for (case, text, ready, message) in [
                    ("short", "我们明天上午去上海。", true, "测试提示"),
                    (
                        "streaming",
                        "hello 😀 中文",
                        false,
                        "正在识别，请稍候；按 Esc 取消。",
                    ),
                    (
                        "warning",
                        "hello 😀 中文",
                        true,
                        "部分音频识别失败，请检查文字后按 Enter 确认，或按 Esc 取消。\n无法获取输入目标；可复制文字后手动粘贴。",
                    ),
                    (
                        "error",
                        "",
                        false,
                        "识别服务暂时不可用。\n请检查连接配置，然后重试。\n按 Esc 关闭。",
                    ),
                ] {
                    self.update_text(text, ready, Some(message))?;
                    for dpi in [96, 144, 192] {
                        let mut suggested = RECT::default();
                        ensure!(
                            unsafe { GetWindowRect(self.state.window.get(), &mut suggested) } != 0,
                            "Could not read appearance fixture bounds"
                        );
                        unsafe {
                            SendMessageW(
                                self.state.window.get(),
                                WM_DPICHANGED,
                                dpi | (dpi << 16),
                                (&raw const suggested) as isize,
                            );
                        }
                        for control in [self.state.text.get(), self.state.status.get()] {
                            let dc = unsafe { GetDC(control) };
                            ensure!(!dc.is_null(), "Could not read native control colors");
                            let saved = unsafe { SaveDC(dc) };
                            let colors = unsafe {
                                SetTextColor(dc, 0x00123456);
                                SetBkColor(dc, 0x00654321);
                                SendMessageW(
                                    self.state.window.get(),
                                    if control == self.state.text.get() && ready {
                                        WM_CTLCOLOREDIT
                                    } else {
                                        WM_CTLCOLORSTATIC
                                    },
                                    dc as usize,
                                    control as isize,
                                );
                                (GetTextColor(dc), GetBkColor(dc), GetDCBrushColor(dc))
                            };
                            unsafe {
                                RestoreDC(dc, saved);
                                ReleaseDC(control, dc);
                            }
                            let (foreground, background) = if self.state.style.get().high_contrast {
                                unsafe {
                                    (GetSysColor(COLOR_WINDOWTEXT), GetSysColor(COLOR_WINDOW))
                                }
                            } else if control == self.state.text.get() {
                                (0x00ffffff, 0x001e1e1e)
                            } else {
                                (0x00a4a4a4, 0x001e1e1e)
                            };
                            ensure!(
                                colors == (foreground, background, background),
                                "Wrong native body/status colors for {case}; ready={ready}; DPI={dpi}"
                            );
                        }
                        let mut body = RECT::default();
                        let mut status = RECT::default();
                        let mut window = RECT::default();
                        ensure!(
                            unsafe { GetWindowRect(self.state.text.get(), &mut body) } != 0
                                && unsafe { GetWindowRect(self.state.status.get(), &mut status) }
                                    != 0
                                && unsafe { GetWindowRect(self.state.window.get(), &mut window) }
                                    != 0,
                            "Could not read native body/status bounds"
                        );
                        // Use the STATIC's own font and final width, not a fixed two-line footer.
                        let (measured, _) = self.state.measure_text(
                            self.state.status.get(),
                            Some(status.right - status.left),
                        )?;
                        eprintln!(
                            "native preview footer: case={case}; DPI={dpi}; system-colors={system_colors}; measured-height={}; status-height={}; body-gap={}; bottom-gap={}; side-insets=({}, {}); minimum-height={}",
                            measured.bottom,
                            status.bottom - status.top,
                            status.top - body.bottom,
                            window.bottom - status.bottom,
                            status.left - window.left,
                            window.right - status.right,
                            self.state.pixels(32),
                        );
                        // Preserve the failing frame too; geometry assertions must not erase evidence.
                        if let Some(directory) = artifacts {
                            self.smoke_capture(&directory.join(format!(
                                "appearance-{case}-{dpi}-system-colors-{system_colors}.png"
                            )))?;
                        }
                        // Two warning lines fit the minimum; the three-line error must grow it.
                        ensure!(
                            status.bottom - status.top >= measured.bottom
                                && status.top - body.bottom >= self.state.pixels(12)
                                && window.bottom - status.bottom >= self.state.pixels(12)
                                && status.left - window.left == self.state.pixels(20)
                                && window.right - status.right == self.state.pixels(20)
                                && (case != "error" || measured.bottom > self.state.pixels(32)),
                            "Warning/error footer assertion failed: case={case}; DPI={dpi}; system-colors={system_colors}"
                        );
                    }
                }
            }
            eprintln!(
                "native preview appearance: colors, footer and simulated system-color fallback checked"
            );
            Ok(())
        })();
        self.state.style.set(original);
        result
    }

    pub(crate) fn smoke_streaming(&mut self) -> Result<()> {
        // These checks address our own controls directly and need no foreground activation.
        ensure!(
            unsafe { IsWindow(self.state.window.get()) } != 0,
            "Streaming preview was not created"
        );
        let seed = nul_terminated_utf16("hello 😀 old");
        self.state.set_read_only(false);
        unsafe {
            SendMessageW(self.state.text.get(), EM_SETSEL, 0, -1);
            SendMessageW(
                self.state.text.get(),
                EM_REPLACESEL,
                0,
                seed.as_ptr() as isize,
            );
            // This prefix is before the corrected word and must survive in
            // UTF-16 units even though the later emoji uses a surrogate pair.
            SendMessageW(self.state.text.get(), EM_SETSEL, 0, 6);
        }
        self.state.set_read_only(true);
        self.update_text("hello 😀 new", false, None)?;
        ensure!(
            self.state.normalized_text()? == "hello 😀 new",
            "Streaming patch did not replace only the tail"
        );
        let mut start = 0u32;
        let mut end = 0u32;
        unsafe {
            SendMessageW(
                self.state.text.get(),
                EM_GETSEL,
                (&raw mut start) as usize,
                (&raw mut end) as isize,
            );
        }
        ensure!(
            (start, end) == (0, 6),
            "Streaming patch changed an unaffected selection"
        );
        ensure!(
            unsafe { GetWindowLongPtrW(self.state.text.get(), GWL_STYLE) } as u32
                & ES_READONLY as u32
                != 0,
            "Streaming preview remained editable"
        );
        ensure!(
            unsafe { SendMessageW(self.state.text.get(), EM_CANUNDO, 0, 0) } == 0,
            "Streaming patch entered the undo history"
        );
        self.update_text("", false, None)?;
        ensure!(
            self.state.normalized_text()?.is_empty(),
            "An empty aggregate did not retract the provisional text"
        );
        self.smoke_placement()?;
        self.update_text("hello 😀 final", true, None)?;
        ensure!(
            unsafe { GetWindowLongPtrW(self.state.text.get(), GWL_STYLE) } as u32
                & ES_READONLY as u32
                == 0,
            "Final preview did not become editable"
        );
        Ok(())
    }

    /// Resize the actual controls at each screen corner, both generating and editable.
    fn smoke_placement(&mut self) -> Result<()> {
        let mut bounds = RECT::default();
        ensure!(
            unsafe { GetWindowRect(self.state.window.get(), &mut bounds) } != 0,
            "Could not read bubble bounds"
        );
        let current = PreviewRect::new(
            bounds.left as f64,
            bounds.top as f64,
            (bounds.right - bounds.left) as f64,
            (bounds.bottom - bounds.top) as f64,
        )
        .ok_or_else(|| anyhow!("Invalid bubble bounds"))?;
        let work = position::work_area(current).ok_or_else(|| anyhow!("No preview work area"))?;
        let saved_anchor = self.state.anchor.get();
        let long = "中文 English 🎙️ 自动换行。".repeat(40);
        let result = (|| {
            for x in [work.x + 8.0, work.x + work.width - 9.0] {
                for y in [work.y + 8.0, work.y + work.height - 26.0] {
                    let anchor = PreviewRect::new(x, y, 1.0, 18.0).unwrap();
                    self.state.anchor.set(Some(anchor));
                    for (text, ready, warning, scrollable) in [
                        ("我们明天上午去上海。", false, None, false),
                        (long.as_str(), false, None, true),
                        ("我们明天上午去上海。", true, None, false),
                        (long.as_str(), true, Some("测试提示"), true),
                        (
                            "已编辑的文字：中文、English 和 emoji 🎙️。",
                            true,
                            None,
                            false,
                        ),
                    ] {
                        self.update_text(text, ready, warning)?;
                        // The reported short sentence wrapped because a permanent scrollbar
                        // consumed its measured width. Exercise growth and deletion too.
                        ensure!(
                            (unsafe { GetWindowLongPtrW(self.state.text.get(), GWL_STYLE) }
                                & WS_VSCROLL as isize
                                != 0)
                                == scrollable
                                && (scrollable
                                    || unsafe {
                                        SendMessageW(self.state.text.get(), EM_GETLINECOUNT, 0, 0)
                                    } == 1),
                            "Preview wrapped fitting text or kept the wrong scrollbar state"
                        );
                        ensure!(
                            unsafe { GetWindowRect(self.state.window.get(), &mut bounds) } != 0,
                            "Could not read resized bubble bounds"
                        );
                        ensure!(
                            scrollable
                                || bounds.bottom - bounds.top
                                    == self.state.pixels(28) + self.state.pixels(24),
                            "Fitting text did not return to the single-line bubble height"
                        );
                        let expected = preview_origin(
                            anchor,
                            (
                                (bounds.right - bounds.left) as f64,
                                (bounds.bottom - bounds.top) as f64,
                            ),
                            work,
                        );
                        ensure!(
                            (bounds.left as f64 - expected.0).abs() <= 1.0
                                && (bounds.top as f64 - expected.1).abs() <= 1.0
                                && bounds.left as f64 >= work.x + 7.0
                                && bounds.top as f64 >= work.y + 7.0
                                && bounds.right as f64 <= work.x + work.width - 7.0
                                && bounds.bottom as f64 <= work.y + work.height - 7.0,
                            "Resized preview escaped its work area"
                        );
                    }
                }
            }
            self.update_text(&long, false, None)?;
            let mut viewport = RECT::default();
            unsafe {
                SendMessageW(
                    self.state.text.get(),
                    EM_GETRECT,
                    0,
                    (&raw mut viewport) as isize,
                )
            };
            let (_, line_height) = self.state.measure_text(self.state.text.get(), None)?;
            let fitting_lines = (viewport.bottom - viewport.top) / line_height;
            for ready in [false, true] {
                for lines in [fitting_lines, fitting_lines + 1, fitting_lines] {
                    let text = "我们明天上午去上海。\n".repeat((lines - 1) as usize)
                        + "我们明天上午去上海。";
                    self.update_text(&text, ready, None)?;
                    ensure!(
                        unsafe { SendMessageW(self.state.text.get(), EM_GETLINECOUNT, 0, 0) }
                            == lines as isize
                            && (unsafe { GetWindowLongPtrW(self.state.text.get(), GWL_STYLE) }
                                & WS_VSCROLL as isize
                                != 0)
                                == (lines > fitting_lines),
                        "Scrollbar did not follow the fitting/overflowing line boundary"
                    );
                }
            }
            Ok(())
        })();
        self.state.anchor.set(saved_anchor);
        result
    }

    #[cfg(feature = "integration")]
    pub(crate) fn smoke_confirmation(
        &self,
        edited_text: &str,
        verify_pending: &dyn Fn() -> Result<()>,
    ) -> Result<bool> {
        if unsafe { GetForegroundWindow() } != self.state.window.get() {
            return Ok(false);
        }
        let dpi = unsafe { GetDpiForWindow(self.state.window.get()) };
        ensure!(dpi > 0, "Could not read preview DPI");
        let pixels = |logical: i32| ((i64::from(logical) * i64::from(dpi) + 48) / 96) as i32;
        let replace = |value: &str| unsafe {
            let value = nul_terminated_utf16(value);
            SendMessageW(self.state.text.get(), EM_SETSEL, 0, -1);
            SendMessageW(
                self.state.text.get(),
                EM_REPLACESEL,
                1,
                value.as_ptr() as isize,
            );
        };
        let bounds = || -> Result<RECT> {
            let mut bounds = RECT::default();
            ensure!(
                unsafe { GetWindowRect(self.state.window.get(), &mut bounds) } != 0,
                "Could not read bubble bounds"
            );
            Ok(bounds)
        };
        // Use actual edit notifications and wrapped lines, rather than calling
        // the layout helper directly, so a missing EN_CHANGE handler is caught.
        replace("短句");
        let short = bounds()?;
        replace(&"中文 English 🎙️ 自动换行。".repeat(20));
        let grown = bounds()?;
        ensure!(
            grown.bottom - grown.top > short.bottom - short.top,
            "Bubble did not grow after editing"
        );
        ensure!(
            grown.right - grown.left > short.right - short.left
                && grown.right - grown.left <= pixels(420),
            "Short candidate did not shrink below the wrapping width"
        );
        if let Some(anchor) = self.state.anchor.get() {
            let work =
                position::work_area(anchor).ok_or_else(|| anyhow!("No preview work area"))?;
            let expected = preview_origin(
                anchor,
                (
                    (grown.right - grown.left) as f64,
                    (grown.bottom - grown.top) as f64,
                ),
                work,
            );
            ensure!(
                (grown.left as f64 - expected.0).abs() <= 1.0
                    && (grown.top as f64 - expected.1).abs() <= 1.0,
                "Growing bubble ignored its caret and work area"
            );
        } else {
            ensure!(
                grown.top == short.top,
                "Unanchored bubble top edge moved while growing"
            );
        }
        replace(&"中文 English 🎙️ 自动换行。".repeat(200));
        let capped = bounds()?;
        ensure!(
            capped.bottom - capped.top <= pixels(220),
            "Long text exceeded the bubble height limit"
        );
        replace("短句\r\n");
        let newline = bounds()?;
        ensure!(
            newline.bottom - newline.top > short.bottom - short.top,
            "Trailing newline did not leave room for the caret"
        );
        replace("短句");
        let shrunk = bounds()?;
        ensure!(
            shrunk.bottom - shrunk.top == short.bottom - short.top,
            "Bubble did not shrink after deletion"
        );
        let mut info = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        ensure!(
            unsafe { GetGUIThreadInfo(0, &mut info) } != 0
                && info.hwndCaret == self.state.text.get(),
            "Could not read the native caret"
        );
        ensure!(
            (info.rcCaret.top + info.rcCaret.bottom - (short.bottom - short.top - pixels(24)))
                .abs()
                <= pixels(2),
            "Short candidate is not vertically centered"
        );
        replace("iiiiiiiiiiiiiiii");
        let narrow = bounds()?;
        replace("WWWWWWWWWWWWWWWW");
        let wide = bounds()?;
        ensure!(
            wide.right - wide.left > narrow.right - narrow.left,
            "Bubble width ignored native glyph widths"
        );
        replace("短句");
        self.state.set_message(Some("测试提示"))?;
        replace(&"自动换行保留提示。".repeat(20));
        ensure!(
            self.state.message_visible.get()
                && unsafe { IsWindowVisible(self.state.status.get()) } != 0,
            "Editing lost the delivery message"
        );
        replace("短句");
        let footer = bounds()?;
        ensure!(
            footer.bottom - footer.top == short.bottom - short.top + pixels(44),
            "Message footer overlapped the resized editor"
        );
        let mut viewport = RECT::default();
        ensure!(
            unsafe { GetClientRect(self.state.text.get(), &mut viewport) } != 0,
            "Could not read editor bounds"
        );
        ensure!(
            viewport.bottom == short.bottom - short.top - pixels(24),
            "Editor did not shrink with its bubble"
        );
        self.state.set_message(None)?;
        verify_pending()?;
        // Exercise the real subclass chain, including key repeats and WM_CHAR.
        unsafe {
            let before = GetWindowTextLengthW(self.state.text.get());
            for _ in 0..5 {
                SendMessageW(
                    self.state.text.get(),
                    WM_KEYDOWN,
                    VK_RETURN as usize,
                    1 << 30,
                );
                SendMessageW(self.state.text.get(), WM_CHAR, VK_RETURN as usize, 1 << 30);
            }
            SendMessageW(
                self.state.text.get(),
                WM_KEYUP,
                VK_RETURN as usize,
                (1 << 30) | (1 << 31),
            );
            verify_pending()?;
            ensure!(
                GetWindowTextLengthW(self.state.text.get()) == before,
                "Inherited Enter repeat changed the editable text"
            );
            let edited_text = nul_terminated_utf16(edited_text);
            SendMessageW(self.state.text.get(), EM_SETSEL, 0, -1);
            SendMessageW(
                self.state.text.get(),
                EM_REPLACESEL,
                1,
                edited_text.as_ptr() as isize,
            );
            for index in 0..5 {
                let repeat = if index == 0 { 0 } else { 1 << 30 };
                SendMessageW(
                    self.state.text.get(),
                    WM_KEYDOWN,
                    VK_RETURN as usize,
                    repeat,
                );
                SendMessageW(self.state.text.get(), WM_CHAR, VK_RETURN as usize, repeat);
            }
            verify_pending()?;
            SendMessageW(self.state.text.get(), WM_KEYUP, VK_RETURN as usize, 0);
        }
        Ok(true)
    }
}
