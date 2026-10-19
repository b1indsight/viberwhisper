//! Native editing and delivery checks for the integration demo.

use super::super::nul_terminated_utf16;
use super::*;
use windows_sys::Win32::UI::Controls::{EM_CANUNDO, EM_GETSEL, EM_REPLACESEL};

impl NativePreview {
    pub(crate) fn smoke_streaming(&mut self) -> Result<()> {
        if unsafe { GetForegroundWindow() } != self.state.window.get() {
            return Ok(());
        }
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
        self.update_text("hello 😀 final", true, None)?;
        ensure!(
            unsafe { GetWindowLongPtrW(self.state.text.get(), GWL_STYLE) } as u32
                & ES_READONLY as u32
                == 0,
            "Final preview did not become editable"
        );
        Ok(())
    }

    pub(crate) fn smoke_confirmation(
        &self,
        edited_text: &str,
        verify_pending: &dyn Fn() -> Result<()>,
    ) -> Result<bool> {
        if unsafe { GetForegroundWindow() } != self.state.window.get() {
            return Ok(false);
        }
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
            grown.right - grown.left > short.right - short.left && grown.right - grown.left <= 420,
            "Short candidate did not shrink below the wrapping width"
        );
        ensure!(
            grown.top == short.top,
            "Bubble top edge moved while growing"
        );
        replace(&"中文 English 🎙️ 自动换行。".repeat(200));
        let capped = bounds()?;
        ensure!(
            capped.bottom - capped.top <= 220,
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
            (info.rcCaret.top + info.rcCaret.bottom - (short.bottom - short.top - 24)).abs() <= 2,
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
            footer.bottom - footer.top == short.bottom - short.top + 48,
            "Message footer overlapped the resized editor"
        );
        let mut viewport = RECT::default();
        ensure!(
            unsafe { GetClientRect(self.state.text.get(), &mut viewport) } != 0,
            "Could not read editor bounds"
        );
        ensure!(
            viewport.bottom == short.bottom - short.top - 24,
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
