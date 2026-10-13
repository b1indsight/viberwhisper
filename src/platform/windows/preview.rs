use std::cell::{Cell, RefCell};
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use anyhow::{Result, anyhow, ensure};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_WINDOW, CreateRoundRectRgn, DEFAULT_GUI_FONT, DeleteObject, GetStockObject, SetWindowRgn,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(feature = "integration")]
use windows_sys::Win32::UI::Controls::EM_REPLACESEL;
use windows_sys::Win32::UI::Controls::{EM_SETLIMITTEXT, EM_SETSEL};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RETURN, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::platform::preview::{
    ConfirmationKey, DeliveryTarget, PreviewAction, PreviewEvent, PreviewKey, PreviewNotify,
};
use crate::session::SessionId;

pub(crate) struct NativeTarget {
    window: HWND,
    focus: HWND,
    thread: u32,
    process: u32,
    title: String,
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
        let mut title = [0u16; 512];
        let count = unsafe { GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32) };
        Ok(Self {
            window,
            focus,
            thread,
            process,
            title: String::from_utf16_lossy(&title[..count.max(0) as usize]),
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
        self.validate()
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
    composing: Cell<bool>,
    retry_allowed: Cell<bool>,
}

impl WindowState {
    fn notify(&self, action: PreviewAction) {
        if let Some(session_id) = self.session_id.get() {
            (self.notify)(PreviewEvent { session_id, action });
        }
    }

    fn key(&self, key: WPARAM, pressed: bool) -> bool {
        let action = match key as u16 {
            VK_RETURN => PreviewKey::Confirm,
            VK_ESCAPE => PreviewKey::Cancel,
            _ => return false,
        };
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
                            let message =
                                super::nul_terminated_utf16("无法读取编辑后的文字，请重试。");
                            unsafe {
                                SetWindowTextW(self.status.get(), message.as_ptr());
                                ShowWindow(self.status.get(), SW_SHOW);
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
        if self.composing.get()
            || [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN]
                .iter()
                .any(|key| unsafe { GetKeyState(*key as i32) } < 0)
        {
            self.keys.borrow_mut().reset();
            return false;
        }
        self.keys.borrow_mut().press(action);
        true
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
    if matches!(message, WM_KEYDOWN | WM_KEYUP) && state.key(wparam, message == WM_KEYDOWN) {
        return 0;
    }
    if message == WM_CHAR {
        let key = match wparam as u16 {
            VK_RETURN => Some(PreviewKey::Confirm),
            VK_ESCAPE => Some(PreviewKey::Cancel),
            _ => None,
        };
        if key.is_some_and(|key| state.keys.borrow().is_pressed(key)) {
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
            WM_CLOSE => {
                state.notify(PreviewAction::Cancel);
                return 0;
            }
            WM_ACTIVATE if wparam as u16 == WA_INACTIVE as u16 => state.keys.borrow_mut().reset(),
            WM_KEYDOWN | WM_KEYUP if state.key(wparam, message == WM_KEYDOWN) => return 0,
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
                composing: Cell::new(false),
                retry_allowed: Cell::new(true),
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
            self.layout(None)?;
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
        warning: Option<&str>,
    ) -> Result<()> {
        if self.state.window.get().is_null() {
            self.create()?;
        }
        self.state.session_id.set(Some(session_id));
        self.state.keys.borrow_mut().reset();
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
        self.layout(warning)?;
        self.focus();
        Ok(())
    }

    pub(crate) fn show_error(&mut self, error: &str, retry_allowed: bool) {
        self.state.retry_allowed.set(retry_allowed);
        if let Err(layout_error) = self.layout(Some(error)) {
            tracing::error!(%layout_error, "Could not display preview error");
        }
        self.focus();
    }

    fn layout(&self, message: Option<&str>) -> Result<()> {
        let mut bounds = RECT::default();
        let window = self.state.window.get();
        unsafe {
            ensure!(GetWindowRect(window, &mut bounds) != 0, "无法获取预览尺寸");
        }
        let width = bounds.right - bounds.left;
        let height = 104 + if message.is_some() { 48 } else { 0 };
        let text = super::nul_terminated_utf16(message.unwrap_or(""));
        unsafe {
            ensure!(
                SetWindowPos(
                    window,
                    null_mut(),
                    0,
                    0,
                    width,
                    height,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE
                ) != 0,
                "无法调整预览尺寸"
            );
            let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, 28, 28);
            ensure!(!region.is_null(), "无法创建气泡边缘");
            if SetWindowRgn(window, region, 1) == 0 {
                DeleteObject(region);
                anyhow::bail!("无法设置气泡边缘");
            }
            // Windows owns the region after successful SetWindowRgn.
            SetWindowTextW(self.state.status.get(), text.as_ptr());
            ShowWindow(
                self.state.status.get(),
                if message.is_some() { SW_SHOW } else { SW_HIDE },
            );
        }
        Ok(())
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
        self.state.keys.borrow_mut().reset();
        self.hide();
        if !self.state.text.get().is_null() {
            let empty = [0u16];
            unsafe {
                SetWindowTextW(self.state.text.get(), empty.as_ptr());
                SetWindowTextW(self.state.status.get(), empty.as_ptr());
            }
        }
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
        // Exercise the real subclass chain, including key repeats and WM_CHAR.
        unsafe {
            let edited_text = super::nul_terminated_utf16(edited_text);
            SendMessageW(self.state.text.get(), EM_SETSEL, 0, -1);
            SendMessageW(
                self.state.text.get(),
                EM_REPLACESEL,
                1,
                edited_text.as_ptr() as isize,
            );
            for _ in 0..5 {
                SendMessageW(self.state.text.get(), WM_KEYDOWN, VK_RETURN as usize, 0);
                SendMessageW(self.state.text.get(), WM_CHAR, VK_RETURN as usize, 0);
            }
            verify_pending()?;
            SendMessageW(self.state.text.get(), WM_KEYUP, VK_RETURN as usize, 0);
        }
        Ok(true)
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
