//! Non-modal, menu-style microphone gain popup on the existing Windows UI thread.

use std::cell::{Cell, RefCell};
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use anyhow::{Result, anyhow, ensure};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_MENU, COLOR_MENUTEXT, CreateFontIndirectW, DeleteObject, GetMonitorInfoW, GetSysColor,
    GetSysColorBrush, HDC, HFONT, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RedrawWindow, SetBkColor, SetTextColor,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    ICC_BAR_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, TB_THUMBTRACK, TBM_SETLINESIZE,
    TBM_SETPAGESIZE, TBM_SETPOS, TBM_SETRANGEMAX, TBM_SETRANGEMIN, TBM_SETTICFREQ, TBS_AUTOTICKS,
    TRACKBAR_CLASSW,
};
use windows_sys::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForWindow, SystemParametersInfoForDpi,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SetFocus, VK_ESCAPE, VK_RETURN, VK_TAB,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::platform::gain::{GainDraft, GainNotify};

const STYLE: u32 = WS_POPUP | WS_BORDER;
const EX_STYLE: u32 = WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_CONTROLPARENT;

// SDK constants: 0.61 omits TBM_GETPOS; static alignment is behind SystemServices,
// which the application otherwise does not need to enable.
const TBM_GETPOS: u32 = WM_USER;
const SS_LEFT: u32 = 0;
const SS_CENTER: u32 = 1;
const SS_RIGHT: u32 = 2;

const ID_TITLE: i32 = 3;
const ID_VALUE: i32 = 4;
const ID_SLIDER: i32 = 5;
const ID_TICK_MIN: i32 = 6;
const ID_TICK_MID: i32 = 7;
const ID_TICK_MAX: i32 = 8;
const ID_NOTICE: i32 = 9;

/// Like the preview font, this remains alive until every child switches fonts or is destroyed.
struct GainFont(HFONT);

impl GainFont {
    fn new(dpi: u32) -> Result<Self> {
        ensure!(dpi > 0, "无法获取音频增益面板 DPI");
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
            ..Default::default()
        };
        // SAFETY: Win32 receives a correctly sized, writable system metrics structure.
        ensure!(
            unsafe {
                SystemParametersInfoForDpi(
                    SPI_GETNONCLIENTMETRICS,
                    metrics.cbSize,
                    (&raw mut metrics).cast(),
                    0,
                    dpi,
                )
            } != 0,
            "无法获取音频增益字体：{}",
            std::io::Error::last_os_error()
        );
        let font = unsafe { CreateFontIndirectW(&metrics.lfMenuFont) };
        ensure!(
            !font.is_null(),
            "无法创建音频增益字体：{}",
            std::io::Error::last_os_error()
        );
        Ok(Self(font))
    }
}

impl Drop for GainFont {
    fn drop(&mut self) {
        // SAFETY: No live control still refers to this owned font.
        unsafe { DeleteObject(self.0) };
    }
}

struct WindowState {
    notify: GainNotify,
    draft: RefCell<GainDraft>,
    window: Cell<HWND>,
    open: Cell<bool>,
    held_keys: Cell<u8>,
    dpi: Cell<u32>,
    pending_dpi: Cell<Option<u32>>,
    laying_out: Cell<bool>,
    ready: Cell<bool>,
    anchor: Cell<POINT>,
    font: RefCell<Option<GainFont>>,
}

impl WindowState {
    fn pixels(&self, logical: i32) -> i32 {
        ((i64::from(logical) * i64::from(self.dpi.get()) + 48) / 96) as i32
    }

    fn apply_dpi(&self, dpi: u32) -> Result<()> {
        if self.dpi.get() == dpi && self.font.borrow().is_some() {
            return Ok(());
        }
        let font = GainFont::new(dpi)?;
        self.dpi.set(dpi);
        let window = self.window.get();
        // SendMessage may re-enter a callback. Never retain a RefCell borrow across Win32.
        for id in ID_TITLE..=ID_NOTICE {
            unsafe { SendMessageW(GetDlgItem(window, id), WM_SETFONT, font.0 as usize, 1) };
        }
        self.font.replace(Some(font));
        Ok(())
    }

    /// Fit the popup to the cursor's monitor, repeating if the move changes its DPI.
    fn layout(&self) -> Result<()> {
        if self.laying_out.replace(true) {
            return Ok(());
        }
        let result = (|| loop {
            let window = self.window.get();
            let dpi = self
                .pending_dpi
                .take()
                .unwrap_or_else(|| unsafe { GetDpiForWindow(window) });
            self.apply_dpi(dpi)?;
            let p = |logical| self.pixels(logical);
            let mut monitor = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: The point is in screen coordinates; monitor is correctly sized.
            ensure!(
                unsafe {
                    GetMonitorInfoW(
                        MonitorFromPoint(self.anchor.get(), MONITOR_DEFAULTTONEAREST),
                        &mut monitor,
                    )
                } != 0,
                "无法获取音频增益面板所在屏幕"
            );
            let work = monitor.rcWork;
            ensure!(
                work.right > work.left && work.bottom > work.top,
                "屏幕没有可用工作区"
            );
            let mut bounds = RECT {
                right: p(320),
                bottom: p(150),
                ..Default::default()
            };
            ensure!(
                unsafe { AdjustWindowRectExForDpi(&mut bounds, STYLE, 0, EX_STYLE, dpi) } != 0,
                "无法计算音频增益面板尺寸"
            );
            let width = (bounds.right - bounds.left).min(work.right - work.left);
            let height = (bounds.bottom - bounds.top).min(work.bottom - work.top);
            let anchor = self.anchor.get();
            let x = anchor
                .x
                .saturating_add(p(6))
                .clamp(work.left, work.right - width);
            let below = anchor.y.saturating_add(p(6));
            let y = if below.saturating_add(height) <= work.bottom {
                below
            } else {
                anchor.y.saturating_sub(height).saturating_sub(p(6))
            }
            .clamp(work.top, work.bottom - height);
            ensure!(
                unsafe {
                    SetWindowPos(
                        window,
                        null_mut(),
                        x,
                        y,
                        width,
                        height,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                } != 0,
                "无法放置音频增益面板"
            );
            let mut client = RECT::default();
            ensure!(
                unsafe { GetClientRect(window, &mut client) } != 0,
                "无法获取音频增益面板尺寸"
            );
            let content = (client.right - p(24)).max(1);
            let rows = [
                (ID_TITLE, p(12), p(10), content, p(22)),
                (ID_VALUE, p(12), p(34), content, p(22)),
                (ID_SLIDER, p(12), p(58), content, p(34)),
                (ID_TICK_MIN, p(20), p(92), p(64), p(22)),
                (ID_TICK_MID, (client.right - p(64)) / 2, p(92), p(64), p(22)),
                (ID_TICK_MAX, client.right - p(84), p(92), p(64), p(22)),
                (ID_NOTICE, p(12), p(118), content, p(24)),
            ];
            for (id, x, y, width, height) in rows {
                ensure!(
                    unsafe {
                        SetWindowPos(
                            GetDlgItem(window, id),
                            null_mut(),
                            x,
                            y,
                            width,
                            height,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        )
                    } != 0,
                    "无法调整音频增益控件"
                );
            }
            // As in preview.rs, a cross-monitor move can synchronously request another DPI.
            if self.pending_dpi.get().is_none() {
                break Ok(());
            }
        })();
        self.laying_out.set(false);
        result
    }

    fn refresh(&self) -> Result<()> {
        let (label, notice) = {
            let draft = self.draft.borrow();
            (draft.value_label(), draft.notice())
        };
        for (id, text) in [(ID_VALUE, label.as_str()), (ID_NOTICE, notice)] {
            let text = super::nul_terminated_utf16(text);
            ensure!(
                unsafe { SetWindowTextW(GetDlgItem(self.window.get(), id), text.as_ptr()) } != 0,
                "无法更新音频增益提示"
            );
        }
        Ok(())
    }

    fn sync(&self) -> Result<()> {
        let position = self.draft.borrow().position();
        let slider = unsafe { GetDlgItem(self.window.get(), ID_SLIDER) };
        // TBM_SETPOS does not generate WM_HSCROLL or turn the clamped initial value into an edit.
        unsafe { SendMessageW(slider, TBM_SETPOS, 1, position as isize) };
        self.refresh()
    }

    fn dismiss(&self) {
        // Disarm before ShowWindow can re-enter WM_ACTIVATE. Closing never rolls back input.
        let was_open = self.open.replace(false);
        self.held_keys.set(0);
        if was_open {
            self.submit();
            unsafe { ShowWindow(self.window.get(), SW_HIDE) };
        }
    }

    fn submit(&self) {
        let gain = self.draft.borrow_mut().submit();
        if let Some(gain) = gain {
            (self.notify)(gain);
        }
    }

    fn key(&self, key: WPARAM, pressed: bool, repeat: bool) -> bool {
        let flag = match key as u16 {
            VK_RETURN => 1,
            VK_ESCAPE => 2,
            VK_TAB if self.open.get() => return true,
            _ => return false,
        };
        if !pressed {
            self.held_keys.set(self.held_keys.get() & !flag);
        } else if self.open.get() && !repeat && self.held_keys.get() & flag == 0 {
            self.dismiss();
        }
        true
    }
}

// SAFETY: NativeGain owns the stable boxed state until after DestroyWindow removes every
// child and subclass. All callbacks and native operations run on the same UI thread.
unsafe extern "system" fn control_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    subclass: usize,
    reference: usize,
) -> LRESULT {
    let state = unsafe { &*(reference as *const WindowState) };
    if matches!(message, WM_KEYDOWN | WM_KEYUP)
        && state.key(
            wparam,
            message == WM_KEYDOWN,
            lparam as usize & (1 << 30) != 0,
        )
    {
        return 0;
    }
    if message == WM_CHAR && matches!(wparam as u16, VK_RETURN | VK_ESCAPE | VK_TAB) {
        return 0;
    }
    if message == WM_NCDESTROY {
        unsafe { RemoveWindowSubclass(window, Some(control_proc), subclass) };
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

// SAFETY: WM_NCCREATE installs a pointer to NativeGain's boxed state. It outlives the
// HWND and is accessed only through shared references with interior mutability.
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
        let state = create.lpCreateParams as *const WindowState;
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, state as isize) };
        unsafe { &*state }.window.set(window);
    }
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const WindowState;
    if !state.is_null() {
        let state = unsafe { &*state };
        match message {
            WM_ACTIVATE if wparam as u16 == WA_INACTIVE as u16 => {
                state.dismiss();
                return 0;
            }
            WM_CLOSE | WM_CANCELMODE => {
                state.dismiss();
                return 0;
            }
            WM_HSCROLL
                if state.open.get()
                    && lparam as HWND == unsafe { GetDlgItem(window, ID_SLIDER) } =>
            {
                let position = unsafe { SendMessageW(lparam as HWND, TBM_GETPOS, 0, 0) } as i32;
                state.draft.borrow_mut().change(position);
                if let Err(error) = state.refresh() {
                    tracing::error!(%error, "Could not display microphone gain draft");
                }
                // Thumb tracking only previews; release, arrows, Home/End and page keys commit.
                if (wparam & 0xffff) as u32 != TB_THUMBTRACK {
                    state.submit();
                }
                return 0;
            }
            WM_KEYDOWN | WM_KEYUP
                if state.key(
                    wparam,
                    message == WM_KEYDOWN,
                    lparam as usize & (1 << 30) != 0,
                ) =>
            {
                return 0;
            }
            WM_DPICHANGED => {
                state.pending_dpi.set(Some((wparam & 0xffff) as u32));
                // Keep the cursor anchor and derive our own menu-sized bounds at the new DPI.
                if state.ready.get()
                    && let Err(error) = state.layout()
                {
                    tracing::error!(%error, "Could not rescale microphone gain popup");
                }
                return 0;
            }
            WM_CTLCOLORSTATIC => unsafe {
                let dc = wparam as HDC;
                SetTextColor(dc, GetSysColor(COLOR_MENUTEXT));
                SetBkColor(dc, GetSysColor(COLOR_MENU));
                return GetSysColorBrush(COLOR_MENU) as isize;
            },
            WM_SYSCOLORCHANGE | WM_THEMECHANGED => unsafe {
                RedrawWindow(
                    window,
                    null(),
                    null_mut(),
                    RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
                );
            },
            WM_NCDESTROY => {
                state.open.set(false);
                state.window.set(null_mut());
                unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0) };
            }
            _ => {}
        }
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Owns one persistent hidden-or-visible HWND; never runs a nested message loop.
pub(crate) struct NativeGain {
    state: Box<WindowState>,
}

impl NativeGain {
    /// Build hidden controls. The listener supplies the actual gain with set_gain before use.
    pub(crate) fn new(
        _menu: &tray_icon::menu::Menu,
        _item: &tray_icon::menu::MenuItem,
        notify: GainNotify,
    ) -> Result<Self> {
        let popup = Self {
            state: Box::new(WindowState {
                notify,
                draft: RefCell::new(GainDraft::new(1.0)),
                window: Cell::new(null_mut()),
                open: Cell::new(false),
                held_keys: Cell::new(0),
                dpi: Cell::new(96),
                pending_dpi: Cell::new(None),
                laying_out: Cell::new(false),
                ready: Cell::new(false),
                anchor: Cell::new(POINT::default()),
                font: RefCell::new(None),
            }),
        };
        popup.create()?;
        Ok(popup)
    }

    fn create(&self) -> Result<()> {
        static CLASS: OnceLock<Result<u16, String>> = OnceLock::new();
        let common = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES,
        };
        ensure!(
            unsafe { InitCommonControlsEx(&common) } != 0,
            "无法初始化音频增益滑块"
        );
        let class_name = super::nul_terminated_utf16("ViberWhisperMicrophoneGainPopup");
        let instance = unsafe { GetModuleHandleW(null()) };
        CLASS
            .get_or_init(|| {
                let class = WNDCLASSW {
                    style: CS_DROPSHADOW,
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance,
                    hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
                    hbrBackground: (COLOR_MENU + 1) as usize as _,
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
            .map_err(|error| anyhow!("无法注册音频增益面板：{error}"))?;
        let state = &*self.state as *const WindowState;
        let title = super::nul_terminated_utf16("音频增益");
        // SAFETY: The stable Box remains owned even if setup fails; Drop destroys partial HWNDs.
        let window = unsafe {
            CreateWindowExW(
                EX_STYLE,
                class_name.as_ptr(),
                title.as_ptr(),
                STYLE,
                0,
                0,
                320,
                150,
                null_mut(),
                null_mut(),
                instance,
                state.cast(),
            )
        };
        ensure!(
            !window.is_null(),
            "无法创建音频增益面板：{}",
            std::io::Error::last_os_error()
        );
        let static_class = super::nul_terminated_utf16("STATIC");
        for (id, text, style) in [
            (ID_TITLE, "音频增益", SS_LEFT),
            (ID_VALUE, "", SS_LEFT),
            (ID_SLIDER, "音频增益 (dB)", WS_TABSTOP | TBS_AUTOTICKS),
            (ID_TICK_MIN, "−10 dB", SS_LEFT),
            (ID_TICK_MID, "0", SS_CENTER),
            (ID_TICK_MAX, "+10 dB", SS_RIGHT),
            (ID_NOTICE, "", SS_LEFT),
        ] {
            let class = if id == ID_SLIDER {
                TRACKBAR_CLASSW
            } else {
                static_class.as_ptr()
            };
            let text = super::nul_terminated_utf16(text);
            let handle = unsafe {
                CreateWindowExW(
                    0,
                    class,
                    text.as_ptr(),
                    WS_CHILD | WS_VISIBLE | style,
                    0,
                    0,
                    0,
                    0,
                    window,
                    id as usize as _,
                    instance,
                    null(),
                )
            };
            ensure!(
                !handle.is_null(),
                "无法创建音频增益控件：{}",
                std::io::Error::last_os_error()
            );
            if style & WS_TABSTOP != 0 {
                ensure!(
                    unsafe { SetWindowSubclass(handle, Some(control_proc), 1, state as usize) }
                        != 0,
                    "无法处理音频增益面板按键"
                );
            }
        }
        unsafe {
            let slider = GetDlgItem(window, ID_SLIDER);
            SendMessageW(slider, TBM_SETRANGEMIN, 0, 0);
            SendMessageW(slider, TBM_SETRANGEMAX, 1, 200);
            SendMessageW(slider, TBM_SETLINESIZE, 0, 1);
            SendMessageW(slider, TBM_SETPAGESIZE, 0, 10);
            SendMessageW(slider, TBM_SETTICFREQ, 100, 0);
        }
        self.state.ready.set(true);
        self.state.sync()?;
        self.state.layout()
    }

    /// Keep the popup open across saves; a late acknowledgement must not reveal a closed popup.
    pub(crate) fn set_gain(&mut self, gain: f32, save_failed: bool) {
        let next = self.state.draft.borrow_mut().set_gain(gain, save_failed);
        if let Err(error) = self.state.sync() {
            tracing::error!(%error, "Could not synchronize microphone gain popup");
        }
        if let Some(gain) = next {
            (self.state.notify)(gain);
        }
    }

    /// Open near the cursor or focus the existing draft without resetting or moving it.
    pub(crate) fn show(&mut self) -> Result<()> {
        let already_open = self.state.open.get();
        if !already_open {
            let mut cursor = POINT::default();
            ensure!(
                unsafe { GetCursorPos(&mut cursor) } != 0,
                "无法获取音频增益面板位置"
            );
            self.state.anchor.set(cursor);
            self.state.layout()?;
            self.state.open.set(true);
            let held = [(VK_RETURN, 1), (VK_ESCAPE, 2)]
                .into_iter()
                .filter(|(key, _)| unsafe { GetKeyState(*key as i32) } < 0)
                .fold(0, |held, (_, flag)| held | flag);
            self.state.held_keys.set(held);
            if let Err(error) = self.state.sync() {
                self.state.dismiss();
                return Err(error);
            }
        }
        let window = self.state.window.get();
        unsafe {
            ShowWindow(window, SW_SHOW);
            SetForegroundWindow(window);
        }
        if !self.state.open.get() || unsafe { GetForegroundWindow() } != window {
            if !already_open {
                self.state.dismiss();
            }
            anyhow::bail!("无法激活音频增益面板，请重试");
        }
        unsafe { SetFocus(GetDlgItem(window, ID_SLIDER)) };
        Ok(())
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.state.open.get()
    }
}

impl Drop for NativeGain {
    fn drop(&mut self) {
        self.state.open.set(false);
        // Destroy children and their callbacks before dropping the boxed state or its font.
        if !self.state.window.get().is_null() {
            unsafe { DestroyWindow(self.state.window.get()) };
        }
    }
}
