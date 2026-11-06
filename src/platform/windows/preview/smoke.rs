//! Native editing and delivery checks for the integration demo.

use super::super::nul_terminated_utf16;
use super::*;
use windows_sys::Win32::UI::Controls::{EM_CANUNDO, EM_GETSEL, EM_REPLACESEL};

impl NativePreview {
    /// Exercise real HWNDs with synthetic DPI notifications; this is not a high-DPI display test.
    #[cfg(test)]
    pub(crate) fn smoke_dpi() -> Result<()> {
        use windows_sys::Win32::Graphics::Gdi::{GetObjectW, LOGFONTW};
        use windows_sys::Win32::UI::HiDpi::{
            AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            GetWindowDpiAwarenessContext, SystemParametersInfoForDpi,
        };

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
        for (ready, warning) in [(false, None), (true, Some("测试提示"))] {
            preview.update_text(&text, ready, warning)?;
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
                first > 0,
                "DPI fixture did not scroll away from the first line"
            );
            for (dpi, reentrant) in [
                (96, false),
                (144, false),
                (192, false),
                (96, false),
                (144, true),
            ] {
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
                    bounds.bottom - bounds.top == pixels(if ready { 268 } else { 220 })
                        && bounds.right - bounds.left <= pixels(420)
                        && edit_bounds.left - bounds.left == pixels(16)
                        && edit_bounds.top - bounds.top == pixels(12)
                        && edit_bounds.bottom - edit_bounds.top == pixels(196)
                        && bounds.top == suggested.top
                        && ((bounds.left + bounds.right) - (suggested.left + suggested.right))
                            .abs()
                            <= 1,
                    "Preview did not apply {dpi}-DPI layout and suggested placement"
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
                    ensure!(
                        unsafe {
                            GetObjectW(font, size_of::<LOGFONTW>() as i32, (&raw mut actual).cast())
                        } == size_of::<LOGFONTW>() as i32
                            && actual.lfHeight == expected.lfMessageFont.lfHeight
                            && actual.lfFaceName == expected.lfMessageFont.lfFaceName,
                        "Preview control kept an unscaled font at {dpi} DPI"
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
                    "native preview: synthetic {dpi} DPI; ready={ready}; reentrant={reentrant}; font/layout/editor state checked"
                );
            }
        }
        preview.smoke_streaming()
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
                    for (text, ready, warning) in [
                        ("短句", false, None),
                        (long.as_str(), false, None),
                        ("短句", true, None),
                        (long.as_str(), true, Some("测试提示")),
                    ] {
                        self.update_text(text, ready, warning)?;
                        ensure!(
                            unsafe { GetWindowRect(self.state.window.get(), &mut bounds) } != 0,
                            "Could not read resized bubble bounds"
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
            footer.bottom - footer.top == short.bottom - short.top + pixels(48),
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
