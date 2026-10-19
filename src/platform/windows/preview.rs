use std::cell::{Cell, RefCell};
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use anyhow::{Result, anyhow, ensure};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_WINDOW, CreateRoundRectRgn, DEFAULT_GUI_FONT, DT_CALCRECT, DT_EDITCONTROL, DT_EXPANDTABS,
    DT_NOPREFIX, DeleteObject, DrawTextW, GetDC, GetStockObject, GetTextMetricsW, InvalidateRect,
    ReleaseDC, SelectObject, SetWindowRgn, TEXTMETRICW,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    EM_EMPTYUNDOBUFFER, EM_GETFIRSTVISIBLELINE, EM_GETLINECOUNT, EM_GETMARGINS, EM_GETRECT,
    EM_GETSEL, EM_LINESCROLL, EM_REPLACESEL, EM_SCROLLCARET, EM_SETLIMITTEXT, EM_SETMODIFY,
    EM_SETREADONLY, EM_SETRECTNP, EM_SETSEL,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RETURN, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::platform::preview::{
    ConfirmationKey, DeliveryTarget, PreviewAction, PreviewEvent, PreviewKey, PreviewNotify,
    PreviewRect, TextPatch, preview_origin, wait_for_activation,
};
use crate::session::SessionId;

#[path = "preview/position.rs"]
mod position;

#[cfg(feature = "integration")]
#[path = "preview/smoke.rs"]
mod smoke;

pub(crate) struct NativeTarget {
    window: HWND,
    focus: HWND,
    thread: u32,
    process: u32,
    title: String,
    anchor: Option<PreviewRect>,
}

fn thread_focus(thread: u32) -> Result<HWND> {
    let mut info = GUITHREADINFO {
        cbSize: size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is correctly sized and live for this call.
    ensure!(
        unsafe { GetGUIThreadInfo(thread, &mut info) } != 0,
        "无法获取原输入框焦点"
    );
    ensure!(!info.hwndFocus.is_null(), "原窗口没有可用的输入焦点");
    Ok(info.hwndFocus)
}

fn reject_password(control: HWND) -> Result<()> {
    let mut name = [0u16; 32];
    // SAFETY: The caller has validated this HWND; `name` is a writable UTF-16 buffer.
    let count = unsafe { GetClassNameW(control, name.as_mut_ptr(), name.len() as i32) };
    if count > 0 && String::from_utf16_lossy(&name[..count as usize]).eq_ignore_ascii_case("Edit") {
        let style = unsafe { GetWindowLongPtrW(control, GWL_STYLE) };
        ensure!(
            style as i32 & ES_PASSWORD == 0,
            "不能向密码输入框写入识别文字"
        );
    }
    Ok(())
}

impl NativeTarget {
    pub(crate) fn capture() -> Result<Self> {
        // SAFETY: This query has no caller-side pointer requirements.
        let window = unsafe { GetForegroundWindow() };
        ensure!(!window.is_null(), "未找到输入目标窗口");
        let mut process = 0;
        let thread = unsafe { GetWindowThreadProcessId(window, &mut process) };
        ensure!(
            thread != 0 && process != std::process::id(),
            "请先在目标应用中选择输入框"
        );
        let focus = thread_focus(thread)?;
        reject_password(focus)?;
        let anchor = position::capture_anchor(thread, focus);
        let mut title = [0u16; 512];
        let count = unsafe { GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32) };
        Ok(Self {
            window,
            focus,
            thread,
            process,
            title: String::from_utf16_lossy(&title[..count.max(0) as usize]),
            anchor,
        })
    }

    fn valid_window(&self) -> Result<()> {
        let mut process = 0;
        // SAFETY: Win32 rejects stale handles; the process output is a live pointer.
        let thread = unsafe { GetWindowThreadProcessId(self.window, &mut process) };
        ensure!(
            thread == self.thread
                && process == self.process
                && unsafe { IsWindow(self.focus) } != 0,
            "原输入窗口已关闭，请复制文字后手动输入"
        );
        Ok(())
    }
}

impl DeliveryTarget for NativeTarget {
    fn preview_anchor(&self) -> Option<PreviewRect> {
        self.anchor
    }
    fn name(&self) -> String {
        if self.title.is_empty() {
            "原窗口".into()
        } else {
            self.title.clone()
        }
    }

    fn restore(&self) -> Result<()> {
        self.valid_window()?;
        // SAFETY: The saved window identity was just validated. Failure is surfaced,
        // without attaching input threads or bypassing the foreground restrictions.
        unsafe {
            if IsIconic(self.window) != 0 {
                ShowWindow(self.window, SW_RESTORE);
            }
            ensure!(
                SetForegroundWindow(self.window) != 0,
                "无法恢复原窗口，请重试或复制文字后手动输入"
            );
        }
        wait_for_activation(|| self.validate())
    }

    fn validate(&self) -> Result<()> {
        self.valid_window()?;
        ensure!(
            unsafe { GetForegroundWindow() } == self.window,
            "输入目标已切换，请重试或复制文字"
        );
        ensure!(
            thread_focus(self.thread)? == self.focus,
            "原输入框已变化，请复制文字后手动输入"
        );
        reject_password(self.focus)
    }
}

struct WindowState {
    notify: PreviewNotify,
    session_id: Cell<Option<SessionId>>,
    window: Cell<HWND>,
    text: Cell<HWND>,
    status: Cell<HWND>,
    keys: RefCell<ConfirmationKey>,
    held_keys: Cell<u8>,
    composing: Cell<bool>,
    retry_allowed: Cell<bool>,
    message_visible: Cell<bool>,
    laying_out: Cell<bool>,
    max_width: Cell<i32>,
    anchor: Cell<Option<PreviewRect>>,
    positioned: Cell<bool>,
}

impl WindowState {
    fn normalized_text(&self) -> Result<String> {
        let length = unsafe { GetWindowTextLengthW(self.text.get()) };
        let mut text = vec![0u16; length as usize + 1];
        let count =
            unsafe { GetWindowTextW(self.text.get(), text.as_mut_ptr(), text.len() as i32) };
        ensure!(length == 0 || count > 0, "无法读取预览文字");
        Ok(String::from_utf16_lossy(&text[..count as usize]).replace("\r\n", "\n"))
    }

    fn set_read_only(&self, read_only: bool) {
        unsafe {
            SendMessageW(self.text.get(), EM_SETREADONLY, read_only as usize, 0);
        }
    }

    fn clear_programmatic_undo(&self) {
        unsafe {
            SendMessageW(self.text.get(), EM_EMPTYUNDOBUFFER, 0, 0);
            SendMessageW(self.text.get(), EM_SETMODIFY, 0, 0);
        }
    }

    fn at_scroll_tail(&self) -> Result<bool> {
        let first = unsafe { SendMessageW(self.text.get(), EM_GETFIRSTVISIBLELINE, 0, 0) } as i32;
        let lines = unsafe { SendMessageW(self.text.get(), EM_GETLINECOUNT, 0, 0) } as i32;
        let mut viewport = RECT::default();
        ensure!(
            unsafe { GetClientRect(self.text.get(), &mut viewport) } != 0,
            "无法获取输入框尺寸"
        );
        let (_, line_height) = self.text_metrics()?;
        Ok(first + (viewport.bottom / line_height.max(1)) >= lines)
    }

    fn scroll_to_tail(&self) {
        let length = unsafe { GetWindowTextLengthW(self.text.get()) } as usize;
        unsafe {
            SendMessageW(self.text.get(), EM_SETSEL, length, length as isize);
            SendMessageW(self.text.get(), EM_SCROLLCARET, 0, 0);
        }
    }

    fn disarm_held_keys(&self) {
        let held = [(VK_RETURN, 1), (VK_ESCAPE, 2)]
            .into_iter()
            .filter(|(key, _)| unsafe { GetKeyState(*key as i32) } < 0)
            .fold(0, |held, (_, flag)| held | flag);
        self.held_keys.set(held);
    }

    fn set_message(&self, message: Option<&str>) -> Result<()> {
        let text = super::nul_terminated_utf16(message.unwrap_or(""));
        self.message_visible.set(message.is_some());
        unsafe {
            SetWindowTextW(self.status.get(), text.as_ptr());
            ShowWindow(
                self.status.get(),
                if message.is_some() { SW_SHOW } else { SW_HIDE },
            );
        }
        self.layout()
    }

    fn text_metrics(&self) -> Result<(i32, i32)> {
        let length = unsafe { GetWindowTextLengthW(self.text.get()) };
        let mut text = vec![0u16; length as usize + 1];
        let count =
            unsafe { GetWindowTextW(self.text.get(), text.as_mut_ptr(), text.len() as i32) };
        ensure!(length == 0 || count > 0, "无法测量预览文字");
        // Match the edit control's actual font; the screen DC's default font can differ.
        let font = unsafe { SendMessageW(self.text.get(), WM_GETFONT, 0, 0) } as _;
        let dc = unsafe { GetDC(self.text.get()) };
        ensure!(!dc.is_null(), "无法测量预览文字");
        let previous = unsafe { SelectObject(dc, font) };
        let mut metrics = TEXTMETRICW::default();
        let measured = !previous.is_null() && unsafe { GetTextMetricsW(dc, &mut metrics) } != 0;
        let mut bounds = RECT::default();
        let drawn = if measured && count > 0 {
            unsafe {
                DrawTextW(
                    dc,
                    text.as_ptr(),
                    count,
                    &mut bounds,
                    DT_CALCRECT | DT_NOPREFIX | DT_EXPANDTABS | DT_EDITCONTROL,
                )
            }
        } else {
            0
        };
        unsafe {
            if !previous.is_null() {
                SelectObject(dc, previous);
            }
            ReleaseDC(self.text.get(), dc);
        }
        ensure!(
            measured && metrics.tmHeight > 0 && (count == 0 || drawn > 0),
            "无法测量预览字体"
        );
        Ok((bounds.right, metrics.tmHeight))
    }

    /// Fit short candidates and center their input line, wrapping at the width limit.
    fn layout(&self) -> Result<()> {
        // Resizing a native edit control can synchronously send more notifications.
        if self.laying_out.replace(true) {
            return Ok(());
        }
        let result = (|| {
            let mut bounds = RECT::default();
            let window = self.window.get();
            ensure!(
                unsafe { GetWindowRect(window, &mut bounds) } != 0,
                "无法获取预览尺寸"
            );
            let work = self.anchor.get().and_then(position::work_area);
            let max_width = work.map_or(self.max_width.get(), |work| {
                420.min((work.width as i32 - 16).max(96))
            });
            let (natural_width, line_height) = self.text_metrics()?;
            let margins = unsafe { SendMessageW(self.text.get(), EM_GETMARGINS, 0, 0) } as u32;
            let padding = 34 + (margins & 0xffff) as i32 + (margins >> 16) as i32;
            let width = if self.message_visible.get() {
                max_width
            } else {
                natural_width.saturating_add(padding).clamp(96, max_width)
            };
            let mut viewport = RECT::default();
            ensure!(
                unsafe { GetClientRect(self.text.get(), &mut viewport) } != 0,
                "无法获取输入框尺寸"
            );
            // Reflow at the new width before counting native wrapped lines.
            if viewport.right != width - 32 {
                ensure!(
                    unsafe {
                        SetWindowPos(
                            self.text.get(),
                            null_mut(),
                            16,
                            12,
                            width - 32,
                            viewport.bottom,
                            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOREDRAW,
                        )
                    } != 0,
                    "无法调整预览输入框"
                );
            }
            unsafe {
                SendMessageW(self.text.get(), EM_SETRECTNP, 0, 0);
            }
            let lines = unsafe { SendMessageW(self.text.get(), EM_GETLINECOUNT, 0, 0) } as i32;
            let content_height = lines.max(1).saturating_mul(line_height);
            // Match the macOS viewport bounds; taller text stays scrollable.
            let editor_height = content_height.clamp(28, 196);
            let height = editor_height + 24 + if self.message_visible.get() { 48 } else { 0 };
            let origin = (!self.positioned.get())
                .then(|| {
                    self.anchor.get().zip(work).map(|(anchor, work)| {
                        preview_origin(anchor, (width as f64, height as f64), work)
                    })
                })
                .flatten();
            unsafe {
                ensure!(
                    SetWindowPos(
                        window,
                        null_mut(),
                        origin.map_or(
                            bounds.left + (bounds.right - bounds.left) / 2 - width / 2,
                            |(x, _)| x.round() as i32
                        ),
                        origin.map_or(bounds.top, |(_, y)| y.round() as i32),
                        width,
                        height,
                        SWP_NOZORDER | SWP_NOACTIVATE
                    ) != 0,
                    "无法调整预览尺寸"
                );
                ensure!(
                    SetWindowPos(
                        self.text.get(),
                        null_mut(),
                        16,
                        12,
                        width - 32,
                        editor_height,
                        SWP_NOZORDER | SWP_NOACTIVATE
                    ) != 0,
                    "无法调整预览输入框"
                );
                SendMessageW(self.text.get(), EM_SETRECTNP, 0, 0);
                let mut format = RECT::default();
                SendMessageW(self.text.get(), EM_GETRECT, 0, (&raw mut format) as isize);
                let inset = ((format.bottom - format.top - content_height) / 2).max(0);
                format.top += inset;
                format.bottom -= inset;
                SendMessageW(
                    self.text.get(),
                    EM_SETRECTNP,
                    0,
                    (&raw const format) as isize,
                );
                InvalidateRect(self.text.get(), null(), 1);
                ensure!(
                    SetWindowPos(
                        self.status.get(),
                        null_mut(),
                        16,
                        editor_height + 24,
                        width - 32,
                        36,
                        SWP_NOZORDER | SWP_NOACTIVATE
                    ) != 0,
                    "无法调整预览提示"
                );
                let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, 28, 28);
                ensure!(!region.is_null(), "无法创建气泡边缘");
                if SetWindowRgn(window, region, 1) == 0 {
                    DeleteObject(region);
                    anyhow::bail!("无法设置气泡边缘");
                }
                // Windows owns the region after successful SetWindowRgn.
            }
            self.positioned.set(true);
            Ok(())
        })();
        self.laying_out.set(false);
        result
    }

    fn notify(&self, action: PreviewAction) {
        if let Some(session_id) = self.session_id.get() {
            (self.notify)(PreviewEvent { session_id, action });
        }
    }

    fn key(&self, key: WPARAM, pressed: bool, is_repeat: bool) -> bool {
        let (action, held_flag) = match key as u16 {
            VK_RETURN => (PreviewKey::Confirm, 1),
            VK_ESCAPE => (PreviewKey::Cancel, 2),
            _ => return false,
        };
        if self.held_keys.get() & held_flag != 0 {
            if !pressed {
                self.held_keys.set(self.held_keys.get() & !held_flag);
            }
            return true;
        }
        if !pressed {
            let action = self.keys.borrow_mut().release(action);
            if let Some(action) = action {
                match action {
                    PreviewKey::Confirm if self.retry_allowed.get() => {
                        let length = unsafe { GetWindowTextLengthW(self.text.get()) };
                        let mut text = vec![0u16; length as usize + 1];
                        let count = unsafe {
                            GetWindowTextW(self.text.get(), text.as_mut_ptr(), text.len() as i32)
                        };
                        if length > 0 && count == 0 {
                            if let Err(error) =
                                self.set_message(Some("无法读取编辑后的文字，请重试。"))
                            {
                                tracing::error!(%error, "Could not display preview error");
                            }
                        } else {
                            self.notify(PreviewAction::Confirm(
                                String::from_utf16_lossy(&text[..count as usize])
                                    .replace("\r\n", "\n"),
                            ));
                        }
                    }
                    PreviewKey::Cancel => self.notify(PreviewAction::Cancel),
                    PreviewKey::Confirm => {}
                }
                return true;
            }
            return false;
        }
        if self.modified_or_composing() {
            self.keys.borrow_mut().reset();
            return false;
        }
        self.keys.borrow_mut().press(action, is_repeat);
        true
    }

    fn modified_or_composing(&self) -> bool {
        self.composing.get()
            || [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
                .iter()
                .any(|key| unsafe { GetKeyState(*key as i32) } < 0)
    }
}

// SAFETY: `reference` points to a stable Box<WindowState> owned by NativePreview.
// The window and all its controls are destroyed before that Box is released.
unsafe extern "system" fn control_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass: usize,
    reference: usize,
) -> LRESULT {
    let state = unsafe { &*(reference as *const WindowState) };
    if matches!(message, WM_IME_STARTCOMPOSITION | WM_IME_ENDCOMPOSITION) {
        state.composing.set(message == WM_IME_STARTCOMPOSITION);
        state.keys.borrow_mut().reset();
    }
    if matches!(message, WM_KEYDOWN | WM_KEYUP)
        && state.key(
            wparam,
            message == WM_KEYDOWN,
            lparam as usize & (1 << 30) != 0,
        )
    {
        return 0;
    }
    if message == WM_CHAR {
        // TranslateMessage also posts WM_CHAR for repeats we consumed without
        // arming. Those must not silently insert a newline into the candidate.
        if matches!(wparam as u16, VK_RETURN | VK_ESCAPE) && !state.modified_or_composing() {
            return 0;
        }
    }
    if message == WM_NCDESTROY {
        unsafe {
            RemoveWindowSubclass(window, Some(control_proc), subclass);
        }
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

// SAFETY: WM_NCCREATE installs the stable state pointer. Later callbacks borrow it
// without mutable aliasing; native calls run on the winit UI thread.
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        let state = create.lpCreateParams as *const WindowState;
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, state as isize);
        }
        unsafe { &*state }.window.set(window);
    }
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const WindowState;
    if !state.is_null() {
        let state = unsafe { &*state };
        match message {
            WM_COMMAND
                if (wparam >> 16) as u16 == EN_CHANGE as u16
                    && lparam as HWND == state.text.get() =>
            {
                if let Err(error) = state.layout() {
                    tracing::error!(%error, "Could not resize edited preview");
                }
                return 0;
            }
            WM_CLOSE => {
                state.notify(PreviewAction::Cancel);
                return 0;
            }
            WM_ACTIVATE if wparam as u16 == WA_INACTIVE as u16 => state.keys.borrow_mut().reset(),
            WM_KEYDOWN | WM_KEYUP
                if state.key(
                    wparam,
                    message == WM_KEYDOWN,
                    lparam as usize & (1 << 30) != 0,
                ) =>
            {
                return 0;
            }
            WM_NCDESTROY => unsafe {
                SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            },
            _ => {}
        }
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

pub(crate) struct NativePreview {
    state: Box<WindowState>,
}

impl NativePreview {
    pub(crate) fn new(notify: PreviewNotify) -> Self {
        Self {
            state: Box::new(WindowState {
                notify,
                session_id: Cell::new(None),
                window: Cell::new(null_mut()),
                text: Cell::new(null_mut()),
                status: Cell::new(null_mut()),
                keys: RefCell::new(ConfirmationKey::default()),
                held_keys: Cell::new(0),
                composing: Cell::new(false),
                retry_allowed: Cell::new(true),
                message_visible: Cell::new(false),
                laying_out: Cell::new(false),
                max_width: Cell::new(420),
                anchor: Cell::new(None),
                positioned: Cell::new(false),
            }),
        }
    }

    fn create(&mut self) -> Result<()> {
        static CLASS: OnceLock<Result<u16, String>> = OnceLock::new();
        let class_name = super::nul_terminated_utf16("ViberWhisperConfirmationPreview");
        let instance = unsafe { GetModuleHandleW(null()) };
        let registered = CLASS.get_or_init(|| {
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
                hbrBackground: (COLOR_WINDOW + 1) as _,
                lpszClassName: class_name.as_ptr(),
                ..Default::default()
            };
            let atom = unsafe { RegisterClassW(&class) };
            if atom == 0 {
                Err(std::io::Error::last_os_error().to_string())
            } else {
                Ok(atom)
            }
        });
        registered
            .as_ref()
            .map_err(|error| anyhow!("无法创建预览窗口：{error}"))?;
        let title = super::nul_terminated_utf16("ViberWhisper · 待确认");
        let mut work = RECT {
            left: 0,
            top: 0,
            right: unsafe { GetSystemMetrics(SM_CXSCREEN) },
            bottom: unsafe { GetSystemMetrics(SM_CYSCREEN) },
        };
        unsafe {
            SystemParametersInfoW(SPI_GETWORKAREA, 0, (&raw mut work).cast(), 0);
        }
        let width = 420.min((work.right - work.left - 20).max(240));
        self.state.max_width.set(width);
        let height = 104;
        let x = work.left + (work.right - work.left - width) / 2;
        let y = work.top + (work.bottom - work.top - height) / 2;
        let state = &*self.state as *const WindowState;
        // SAFETY: State is boxed and outlives this HWND and all child callbacks.
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_CONTROLPARENT,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                x,
                y,
                width,
                height,
                null_mut(),
                null_mut(),
                instance,
                state.cast(),
            )
        };
        ensure!(
            !window.is_null(),
            "无法创建预览窗口：{}",
            std::io::Error::last_os_error()
        );
        let setup = (|| {
            let control =
                |class: &str, text: &str, style, id: u16, x, y, width, height| -> Result<HWND> {
                    let class = super::nul_terminated_utf16(class);
                    let text = super::nul_terminated_utf16(text);
                    let handle = unsafe {
                        CreateWindowExW(
                            0,
                            class.as_ptr(),
                            text.as_ptr(),
                            WS_CHILD | WS_VISIBLE | style,
                            x,
                            y,
                            width,
                            height,
                            window,
                            id as usize as _,
                            instance,
                            null(),
                        )
                    };
                    ensure!(
                        !handle.is_null(),
                        "无法创建预览控件：{}",
                        std::io::Error::last_os_error()
                    );
                    unsafe {
                        SendMessageW(
                            handle,
                            WM_SETFONT,
                            GetStockObject(DEFAULT_GUI_FONT) as usize,
                            1,
                        );
                        ensure!(
                            SetWindowSubclass(handle, Some(control_proc), 1, state as usize) != 0,
                            "无法处理预览确认按键"
                        );
                    }
                    Ok(handle)
                };
            self.state
                .status
                .set(control("STATIC", "", 0, 4, 16, height - 4, width - 32, 44)?);
            self.state.text.set(control(
                "EDIT",
                "",
                WS_TABSTOP | WS_VSCROLL | (ES_MULTILINE | ES_WANTRETURN | ES_AUTOVSCROLL) as u32,
                5,
                16,
                12,
                width - 32,
                height - 24,
            )?);
            unsafe {
                SendMessageW(self.state.text.get(), EM_SETLIMITTEXT, 0x7ffffffe, 0);
                ShowWindow(self.state.status.get(), SW_HIDE);
            }
            self.state.set_message(None)?;
            Ok(())
        })();
        if setup.is_err() {
            unsafe {
                DestroyWindow(window);
            }
            self.state.window.set(null_mut());
        }
        setup
    }

    pub(crate) fn show(
        &mut self,
        session_id: SessionId,
        text: &str,
        destination: &str,
        anchor: Option<PreviewRect>,
        warning: Option<&str>,
    ) -> Result<()> {
        if self.state.window.get().is_null() {
            self.create()?;
        }
        self.state.session_id.set(Some(session_id));
        self.state.anchor.set(anchor);
        self.state.positioned.set(false);
        self.state.keys.borrow_mut().reset();
        self.state.held_keys.set(0);
        self.state.retry_allowed.set(true);
        self.state.composing.set(false);
        let title = super::nul_terminated_utf16(&format!("ViberWhisper · {destination}"));
        let text = super::nul_terminated_utf16(&text.replace("\r\n", "\n").replace('\n', "\r\n"));
        unsafe {
            ensure!(
                SetWindowTextW(self.state.text.get(), text.as_ptr()) != 0,
                "无法显示识别文字"
            );
            SetWindowTextW(self.state.window.get(), title.as_ptr());
            SendMessageW(
                self.state.text.get(),
                EM_SETSEL,
                text.len() - 1,
                (text.len() - 1) as isize,
            );
        }
        self.state.set_read_only(false);
        self.state.clear_programmatic_undo();
        self.state.set_message(warning)?;
        if anchor.is_none() {
            position::center(self.state.window.get());
        }
        self.state.scroll_to_tail();
        self.focus();
        Ok(())
    }

    /// Show provisional transcription without taking focus unless the recording has stopped.
    pub(crate) fn show_streaming(
        &mut self,
        session_id: SessionId,
        text: &str,
        destination: &str,
        anchor: Option<PreviewRect>,
        activate: bool,
    ) -> Result<()> {
        if self.state.window.get().is_null() {
            self.create()?;
        }
        self.state.session_id.set(Some(session_id));
        self.state.anchor.set(anchor);
        self.state.positioned.set(false);
        self.state.keys.borrow_mut().reset();
        self.state.held_keys.set(0);
        self.state.retry_allowed.set(false);
        self.state.composing.set(false);
        let title = super::nul_terminated_utf16(&format!("ViberWhisper · {destination}"));
        let text = super::nul_terminated_utf16(&text.replace("\r\n", "\n").replace('\n', "\r\n"));
        unsafe {
            ensure!(
                SetWindowTextW(self.state.text.get(), text.as_ptr()) != 0,
                "无法显示识别文字"
            );
            SetWindowTextW(self.state.window.get(), title.as_ptr());
            SendMessageW(
                self.state.text.get(),
                EM_SETSEL,
                text.len() - 1,
                (text.len() - 1) as isize,
            );
        }
        self.state.set_read_only(true);
        self.state.clear_programmatic_undo();
        self.state.set_message(None)?;
        if anchor.is_none() {
            position::center(self.state.window.get());
        }
        self.state.scroll_to_tail();
        if activate {
            self.focus();
        } else {
            unsafe {
                ShowWindow(self.state.window.get(), SW_SHOWNOACTIVATE);
            }
        }
        Ok(())
    }

    /// Apply a word-aligned tail patch while preserving selection and user scroll state.
    pub(crate) fn update_text(
        &mut self,
        text: &str,
        ready: bool,
        warning: Option<&str>,
    ) -> Result<()> {
        let old = self.state.normalized_text()?;
        let text = text.replace("\r\n", "\n");
        let mut restore_selection = None;
        let mut first_visible_line = None;
        let mut follow_tail = false;
        if let Some(patch) = TextPatch::between(&old, &text) {
            let prefix = &old[..patch.start];
            let prefix_units = prefix
                .encode_utf16()
                .map(|unit| if unit == b'\n' as u16 { 2 } else { 1 })
                .sum::<usize>();
            let replacement = patch.replacement.replace('\n', "\r\n");
            let replacement_units = replacement.encode_utf16().count();
            let replacement_utf16 = super::nul_terminated_utf16(&replacement);
            let mut selection_start = 0u32;
            let mut selection_end = 0u32;
            unsafe {
                SendMessageW(
                    self.state.text.get(),
                    EM_GETSEL,
                    (&raw mut selection_start) as usize,
                    (&raw mut selection_end) as isize,
                );
            }
            let first =
                unsafe { SendMessageW(self.state.text.get(), EM_GETFIRSTVISIBLELINE, 0, 0) } as i32;
            follow_tail = self.state.at_scroll_tail()?;
            if !follow_tail {
                first_visible_line = Some(first);
            }
            // EM_REPLACESEL accepts a programmatic replacement while read-only is
            // temporarily lifted; callers all run on the UI thread, so no user input can race it.
            self.state.set_read_only(false);
            unsafe {
                SendMessageW(self.state.text.get(), EM_SETSEL, prefix_units, -1);
                SendMessageW(
                    self.state.text.get(),
                    EM_REPLACESEL,
                    0,
                    replacement_utf16.as_ptr() as isize,
                );
            }
            let replacement_end = prefix_units + replacement_units;
            let map_selection = |index: u32| {
                if index as usize <= prefix_units {
                    index as usize
                } else {
                    replacement_end
                }
            };
            restore_selection =
                Some((map_selection(selection_start), map_selection(selection_end)));
            self.state.clear_programmatic_undo();
        }
        self.state.set_message(warning)?;
        if let Some((start, end)) = restore_selection {
            if follow_tail {
                self.state.scroll_to_tail();
            } else if let Some(first) = first_visible_line {
                let current =
                    unsafe { SendMessageW(self.state.text.get(), EM_GETFIRSTVISIBLELINE, 0, 0) }
                        as i32;
                unsafe {
                    SendMessageW(
                        self.state.text.get(),
                        EM_LINESCROLL,
                        0,
                        (first - current) as isize,
                    );
                }
            }
            unsafe {
                SendMessageW(self.state.text.get(), EM_SETSEL, start, end as isize);
            }
        }
        if ready {
            self.state.keys.borrow_mut().reset();
            self.state.disarm_held_keys();
            self.state.retry_allowed.set(true);
            self.state.set_read_only(false);
            self.state.clear_programmatic_undo();
        } else {
            self.state.retry_allowed.set(false);
            self.state.set_read_only(true);
        }
        Ok(())
    }

    pub(crate) fn show_error(&mut self, error: &str, retry_allowed: bool) {
        self.state.retry_allowed.set(retry_allowed);
        if let Err(layout_error) = self.state.set_message(Some(error)) {
            tracing::error!(%layout_error, "Could not display preview error");
        }
        self.focus();
    }

    pub(crate) fn focus(&self) {
        let window = self.state.window.get();
        if window.is_null() {
            return;
        }
        unsafe {
            ShowWindow(window, SW_SHOW);
            if SetForegroundWindow(window) != 0 {
                SetFocus(self.state.text.get());
            }
        }
    }
    pub(crate) fn hide(&self) {
        if !self.state.window.get().is_null() {
            unsafe {
                ShowWindow(self.state.window.get(), SW_HIDE);
            }
        }
    }
    pub(crate) fn close(&mut self) {
        self.state.session_id.set(None);
        self.state.anchor.set(None);
        self.state.positioned.set(false);
        self.state.keys.borrow_mut().reset();
        self.state.held_keys.set(0);
        self.hide();
        if !self.state.text.get().is_null() {
            let empty = [0u16];
            unsafe {
                SetWindowTextW(self.state.text.get(), empty.as_ptr());
                SetWindowTextW(self.state.status.get(), empty.as_ptr());
            }
        }
    }
}

impl Drop for NativePreview {
    fn drop(&mut self) {
        self.state.session_id.set(None);
        if !self.state.window.get().is_null() {
            unsafe {
                DestroyWindow(self.state.window.get());
            }
        }
    }
}
