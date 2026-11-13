//! Non-modal, menu-style microphone gain popup on the existing Windows UI thread.
//!
//! It repeats the macOS menu slider's layout and paints itself in the colors of this
//! process's popup menus, so it matches the tray menu that opens it.

use std::cell::{Cell, RefCell};
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use anyhow::{Result, anyhow, ensure};
use windows::Win32::Foundation::RECT as DrawBounds;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT,
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::HDC as DrawDc;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUNDSMALL, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, COLOR_BTNSHADOW, COLOR_HIGHLIGHT, COLOR_MENU, COLOR_MENUTEXT, CreateFontIndirectW,
    DC_BRUSH, DT_CENTER, DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DeleteObject,
    DrawTextW, EndPaint, FillRect, FrameRect, GetMonitorInfoW, GetStockObject, GetSysColor, HDC,
    HFONT, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, PAINTSTRUCT,
    RestoreDC, SaveDC, SelectObject, SetBkMode, SetDCBrushColor, SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::Controls::{
    BPBF_DIB, BeginBufferedPaint, BufferedPaintInit, BufferedPaintUnInit, CloseThemeData,
    EndBufferedPaint, GetThemeColor, MENU_POPUPBACKGROUND, MENU_POPUPBORDERS, MENU_POPUPITEM,
    MPI_NORMAL, OpenThemeData, TMT_BORDERCOLOR, TMT_FILLCOLOR, TMT_TEXTCOLOR,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME,
    VK_LEFT, VK_RETURN, VK_RIGHT, VK_TAB, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::platform::gain::{GainDraft, GainNotify};

const STYLE: u32 = WS_POPUP;
const EX_STYLE: u32 = WS_EX_TOOLWINDOW | WS_EX_TOPMOST;

// Logical layout of the macOS menu view (macos/gain.rs) inside its menu's 5px insets.
const WIDTH: i32 = 312;
const HEIGHT: i32 = 116;
const TRACK_LEFT: i32 = 18;
const TRACK_RIGHT: i32 = 294;
const SLIDER_TOP: i32 = 43;
const SLIDER_BOTTOM: i32 = 67;
const TRACK_HEIGHT: f32 = 6.0;
const KNOB_WIDTH: f32 = 20.0;
const KNOB_HEIGHT: f32 = 16.0;
/// As in NSSlider, the knob stays inside the track, so its center travels this far.
const KNOB_TRAVEL: f32 = (TRACK_RIGHT - TRACK_LEFT) as f32 - KNOB_WIDTH;
// NSTextField draws its text 2px inside the frames that share the slider's edges.
const TEXT_LEFT: i32 = TRACK_LEFT + 2;
const TEXT_RIGHT: i32 = TRACK_RIGHT - 2;
const TEXT_MIDDLE: i32 = (TRACK_LEFT + TRACK_RIGHT) / 2;

/// The system menu font at the popup's DPI, the same face and size the controls used before.
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
        // SAFETY: paint() restores its DC, so no DC still has this owned font selected.
        unsafe { DeleteObject(self.0) };
    }
}

/// Colors of this process's popup menus, which the tray menu is drawn with.
#[derive(Clone, Copy)]
struct Palette {
    background: u32,
    text: u32,
    border: u32,
    track: u32,
    fill: u32,
}

impl Palette {
    fn current(window: HWND) -> Self {
        let fill = unsafe { GetSysColor(COLOR_HIGHLIGHT) };
        if high_contrast() {
            let (background, text) =
                unsafe { (GetSysColor(COLOR_MENU), GetSysColor(COLOR_MENUTEXT)) };
            return Self {
                background,
                text,
                border: text,
                track: text,
                fill,
            };
        }
        // The app never opts into dark popup menus, so the tray menu also uses this theme.
        let class = super::nul_terminated_utf16("MENU");
        let theme = unsafe { OpenThemeData(window, class.as_ptr()) };
        let color = |part, state, property: u32, fallback| {
            let mut value = 0;
            if theme != 0
                && unsafe { GetThemeColor(theme, part, state, property as i32, &mut value) } >= 0
            {
                value
            } else {
                unsafe { GetSysColor(fallback) }
            }
        };
        let background = color(MENU_POPUPBACKGROUND, 0, TMT_FILLCOLOR, COLOR_MENU);
        let text = color(MENU_POPUPITEM, MPI_NORMAL, TMT_TEXTCOLOR, COLOR_MENUTEXT);
        let border = color(MENU_POPUPBORDERS, 0, TMT_BORDERCOLOR, COLOR_BTNSHADOW);
        if theme != 0 {
            unsafe { CloseThemeData(theme) };
        }
        Self {
            background,
            text,
            border,
            // A faint text tint approximates NSSlider's unfilled track on a light menu.
            track: blend(text, background, 30),
            fill,
        }
    }
}

/// Same query as the preview bubble's style; a failed query falls back to system colors.
fn high_contrast() -> bool {
    let mut contrast = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    // Query only; never change the user's global appearance.
    if unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            (&raw mut contrast).cast(),
            0,
        )
    } == 0
    {
        tracing::warn!(error = %std::io::Error::last_os_error(), "Could not query high contrast; using system colors");
        return true;
    }
    contrast.dwFlags & HCF_HIGHCONTRASTON != 0
}

/// Mix `alpha`/255 of `top` over `bottom`, one COLORREF channel at a time.
fn blend(top: u32, bottom: u32, alpha: u32) -> u32 {
    [0, 8, 16].into_iter().fold(0, |color, shift| {
        let channel = |value: u32| (value >> shift) & 0xff;
        color | ((channel(top) * alpha + channel(bottom) * (255 - alpha) + 127) / 255) << shift
    })
}

// Same COLORREF conversion as preview/scrollbar.rs.
fn draw_color(color: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: (color & 0xff) as f32 / 255.0,
        g: ((color >> 8) & 0xff) as f32 / 255.0,
        b: ((color >> 16) & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

/// Logical x of the knob center for a 0–200 slider position.
fn knob_center(position: i32) -> f32 {
    TRACK_LEFT as f32 + KNOB_WIDTH / 2.0 + KNOB_TRAVEL * position as f32 / 200.0
}

/// GDI cannot antialias the round track and knob; a software DC target needs no GPU device.
fn render_target() -> windows::core::Result<ID2D1DCRenderTarget> {
    unsafe {
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        factory.CreateDCRenderTarget(&D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            // 96 DPI makes Direct2D units physical pixels, matching the GDI layout.
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        })
    }
}

/// uxtheme requires buffered painting to stay initialized on the thread that uses it.
struct BufferedPaint;

impl BufferedPaint {
    fn init() -> Result<Self> {
        let result = unsafe { BufferedPaintInit() };
        ensure!(result >= 0, "无法初始化音频增益面板绘制：{result:#x}");
        Ok(Self)
    }
}

impl Drop for BufferedPaint {
    fn drop(&mut self) {
        unsafe { BufferedPaintUnInit() };
    }
}

struct WindowState {
    notify: GainNotify,
    draft: RefCell<GainDraft>,
    window: Cell<HWND>,
    open: Cell<bool>,
    held_keys: Cell<u8>,
    /// Logical offset from the knob center while the mouse drags it.
    grab: Cell<Option<f32>>,
    dpi: Cell<u32>,
    pending_dpi: Cell<Option<u32>>,
    laying_out: Cell<bool>,
    ready: Cell<bool>,
    /// Windows 11 rounds and outlines the popup like its menus; Windows 10 menus are square.
    rounded: Cell<bool>,
    anchor: Cell<POINT>,
    palette: Cell<Palette>,
    font: RefCell<Option<GainFont>>,
    target: RefCell<Option<ID2D1DCRenderTarget>>,
}

impl WindowState {
    fn pixels(&self, logical: i32) -> i32 {
        ((i64::from(logical) * i64::from(self.dpi.get()) + 48) / 96) as i32
    }

    fn logical(&self, pixels: i32) -> f32 {
        pixels as f32 * 96.0 / self.dpi.get() as f32
    }

    fn apply_dpi(&self, dpi: u32) -> Result<()> {
        if self.dpi.get() == dpi && self.font.borrow().is_some() {
            return Ok(());
        }
        let font = GainFont::new(dpi)?;
        self.dpi.set(dpi);
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
            // The popup has no non-client frame, so the window is exactly the painted area.
            let width = p(WIDTH).min(work.right - work.left);
            let height = p(HEIGHT).min(work.bottom - work.top);
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
            // As in preview.rs, a cross-monitor move can synchronously request another DPI.
            if self.pending_dpi.get().is_none() {
                self.repaint();
                break Ok(());
            }
        })();
        self.laying_out.set(false);
        result
    }

    fn repaint(&self) {
        unsafe { InvalidateRect(self.window.get(), null(), 0) };
    }

    /// Paint off-screen so a drag never shows the background between text and slider passes.
    fn paint(&self, target: HDC) {
        let mut client = RECT::default();
        unsafe { GetClientRect(self.window.get(), &mut client) };
        let mut buffered = null_mut();
        let buffer =
            unsafe { BeginBufferedPaint(target, &client, BPBF_DIB, null(), &mut buffered) };
        let dc = if buffer == 0 {
            tracing::warn!("Could not buffer the microphone gain popup; painting directly");
            target
        } else {
            buffered
        };
        let palette = self.palette.get();
        let value = format!("音频增益  {}", self.draft.borrow().value_label());
        let notice = self.draft.borrow().notice();
        let font = self.font.borrow().as_ref().map(|font| font.0);
        let p = |logical| self.pixels(logical);
        // Stock DC objects belong to Windows; the buffer DC state is restored afterwards.
        unsafe {
            let saved = SaveDC(dc);
            SetDCBrushColor(dc, palette.background);
            FillRect(dc, &client, GetStockObject(DC_BRUSH));
            if !self.rounded.get() {
                SetDCBrushColor(dc, palette.border);
                FrameRect(dc, &client, GetStockObject(DC_BRUSH));
            }
            if let Some(font) = font {
                SelectObject(dc, font);
            }
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, palette.text);
            for (text, left, top, right, bottom, align) in [
                (value.as_str(), TEXT_LEFT, 13, TEXT_RIGHT, 35, DT_LEFT),
                ("−10 dB", TEXT_LEFT, 69, TEXT_MIDDLE, 87, DT_LEFT),
                ("0", TEXT_MIDDLE - 30, 69, TEXT_MIDDLE + 30, 87, DT_CENTER),
                ("+10 dB", TEXT_MIDDLE, 69, TEXT_RIGHT, 87, DT_RIGHT),
                (notice, TEXT_LEFT, 87, TEXT_RIGHT, 107, DT_LEFT),
            ] {
                let text = super::nul_terminated_utf16(text);
                let mut bounds = RECT {
                    left: p(left),
                    top: p(top),
                    right: p(right),
                    bottom: p(bottom),
                };
                DrawTextW(
                    dc,
                    text.as_ptr(),
                    -1,
                    &mut bounds,
                    DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | align,
                );
            }
            RestoreDC(dc, saved);
        }
        if let Err(error) = self.paint_slider(dc, palette) {
            // A lost or failed target is recreated on the next paint.
            self.target.replace(None);
            tracing::error!(%error, "Could not draw microphone gain slider");
        }
        if buffer != 0 {
            unsafe { EndBufferedPaint(buffer, 1) };
        }
    }

    fn paint_slider(&self, dc: HDC, palette: Palette) -> windows::core::Result<()> {
        let cached = self.target.borrow().clone();
        let target = match cached {
            Some(target) => target,
            None => {
                let target = render_target()?;
                self.target.replace(Some(target.clone()));
                target
            }
        };
        let bounds = DrawBounds {
            left: self.pixels(TRACK_LEFT),
            top: self.pixels(SLIDER_TOP),
            right: self.pixels(TRACK_RIGHT),
            bottom: self.pixels(SLIDER_BOTTOM),
        };
        // Direct2D coordinates are physical pixels relative to the bound slider rectangle.
        let scale = self.dpi.get() as f32 / 96.0;
        let x = |logical: f32| logical * scale - bounds.left as f32;
        let middle = (SLIDER_TOP + SLIDER_BOTTOM) as f32 / 2.0 * scale - bounds.top as f32;
        let pill = |left: f32, right: f32, height: f32| {
            let radius = height * scale / 2.0;
            D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F {
                    left,
                    top: middle - radius,
                    right,
                    bottom: middle + radius,
                },
                radiusX: radius,
                radiusY: radius,
            }
        };
        let knob = x(knob_center(self.draft.borrow().position()));
        let half_knob = KNOB_WIDTH * scale / 2.0;
        let track = pill(x(TRACK_LEFT as f32), x(TRACK_RIGHT as f32), TRACK_HEIGHT);
        let filled = pill(x(TRACK_LEFT as f32), knob, TRACK_HEIGHT);
        let thumb = pill(knob - half_knob, knob + half_knob, KNOB_HEIGHT);
        // Keep the one-pixel outline inside the knob instead of half outside it.
        let outline = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: thumb.rect.left + 0.5,
                top: thumb.rect.top + 0.5,
                right: thumb.rect.right - 0.5,
                bottom: thumb.rect.bottom - 0.5,
            },
            radiusX: thumb.radiusX - 0.5,
            radiusY: thumb.radiusY - 0.5,
        };
        unsafe {
            target.BindDC(DrawDc(dc as isize), &bounds)?;
            target.BeginDraw();
            let drawn = (|| -> windows::core::Result<()> {
                let brush = |color| target.CreateSolidColorBrush(&draw_color(color), None);
                let background = draw_color(palette.background);
                target.Clear(Some(&raw const background));
                target.FillRoundedRectangle(&track, &brush(palette.track)?);
                target.FillRoundedRectangle(&filled, &brush(palette.fill)?);
                target.FillRoundedRectangle(&thumb, &brush(palette.background)?);
                target.DrawRoundedRectangle(
                    &outline,
                    &brush(palette.border)?,
                    1.0,
                    None::<&ID2D1StrokeStyle>,
                );
                Ok(())
            })();
            // Close the draw even when a brush failed, before reporting either error.
            let ended = target.EndDraw(None, None);
            drawn.and(ended)
        }
    }

    /// Show a new position; keyboard steps commit at once, mouse drags on release.
    fn adjust(&self, position: i32, commit: bool) {
        self.draft.borrow_mut().change(position);
        self.repaint();
        if commit {
            self.submit();
        }
    }

    /// Clicking the track jumps there, as NSSlider does; grabbing the knob does not move it.
    fn press(&self, x: f32, y: f32) {
        let inside = (TRACK_LEFT as f32..=TRACK_RIGHT as f32).contains(&x)
            && (SLIDER_TOP as f32..=SLIDER_BOTTOM as f32).contains(&y);
        if !self.open.get() || !inside {
            return;
        }
        let offset = x - knob_center(self.draft.borrow().position());
        self.grab.set(Some(if offset.abs() <= KNOB_WIDTH / 2.0 {
            offset
        } else {
            0.0
        }));
        unsafe { SetCapture(self.window.get()) };
        self.drag(x);
    }

    fn drag(&self, x: f32) {
        if let Some(grab) = self.grab.get() {
            let position = (x - grab - knob_center(0)) * 200.0 / KNOB_TRAVEL;
            self.adjust(position.round() as i32, false);
        }
    }

    /// Commit a drag on release or when another window takes the mouse capture.
    fn release(&self) {
        if self.grab.take().is_some() {
            // The re-entrant WM_CAPTURECHANGED finds no drag left to commit.
            unsafe { ReleaseCapture() };
            self.submit();
        }
    }

    fn dismiss(&self) {
        // Disarm before ShowWindow can re-enter WM_ACTIVATE. Closing never rolls back input.
        let was_open = self.open.replace(false);
        self.held_keys.set(0);
        if was_open {
            self.release();
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
        let key = key as u16;
        let flag = match key {
            VK_RETURN => 1,
            VK_ESCAPE => 2,
            VK_TAB if self.open.get() => return true,
            // As on macOS, arrows step 0.1 dB and Home/End jump to the limits.
            VK_LEFT | VK_DOWN | VK_RIGHT | VK_UP | VK_HOME | VK_END if self.open.get() => {
                if pressed {
                    let position = self.draft.borrow().position();
                    let position = match key {
                        VK_LEFT | VK_DOWN => position - 1,
                        VK_RIGHT | VK_UP => position + 1,
                        VK_HOME => 0,
                        _ => 200,
                    };
                    self.adjust(position, true);
                }
                return true;
            }
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
        // Signed client coordinates, because a captured drag can leave the popup.
        let point = |shift: u32| state.logical((lparam >> shift) as u16 as i16 as i32);
        match message {
            WM_ACTIVATE if wparam as u16 == WA_INACTIVE as u16 => {
                state.dismiss();
                return 0;
            }
            WM_CLOSE | WM_CANCELMODE => {
                state.dismiss();
                return 0;
            }
            WM_LBUTTONDOWN => {
                state.press(point(0), point(16));
                return 0;
            }
            WM_MOUSEMOVE => {
                state.drag(point(0));
                return 0;
            }
            WM_LBUTTONUP => {
                state.drag(point(0));
                state.release();
                return 0;
            }
            WM_CAPTURECHANGED => {
                state.release();
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
            WM_ERASEBKGND => return 1,
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                let dc = unsafe { BeginPaint(window, &mut paint) };
                state.paint(dc);
                unsafe { EndPaint(window, &paint) };
                return 0;
            }
            WM_SYSCOLORCHANGE | WM_THEMECHANGED => {
                state.palette.set(Palette::current(window));
                state.repaint();
            }
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
    // Declared after state: buffered painting ends only once the window is destroyed.
    _paint: BufferedPaint,
}

impl NativeGain {
    /// Build the hidden popup. The listener supplies the actual gain with set_gain before use.
    pub(crate) fn new(
        _menu: &tray_icon::menu::Menu,
        _item: &tray_icon::menu::MenuItem,
        notify: GainNotify,
    ) -> Result<Self> {
        let popup = Self {
            _paint: BufferedPaint::init()?,
            state: Box::new(WindowState {
                notify,
                draft: RefCell::new(GainDraft::new(1.0)),
                window: Cell::new(null_mut()),
                open: Cell::new(false),
                held_keys: Cell::new(0),
                grab: Cell::new(None),
                dpi: Cell::new(96),
                pending_dpi: Cell::new(None),
                laying_out: Cell::new(false),
                ready: Cell::new(false),
                rounded: Cell::new(false),
                anchor: Cell::new(POINT::default()),
                palette: Cell::new(Palette {
                    background: 0,
                    text: 0,
                    border: 0,
                    track: 0,
                    fill: 0,
                }),
                font: RefCell::new(None),
                target: RefCell::new(None),
            }),
        };
        popup.create()?;
        Ok(popup)
    }

    fn create(&self) -> Result<()> {
        static CLASS: OnceLock<Result<u16, String>> = OnceLock::new();
        let class_name = super::nul_terminated_utf16("ViberWhisperMicrophoneGainPopup");
        let instance = unsafe { GetModuleHandleW(null()) };
        CLASS
            .get_or_init(|| {
                let class = WNDCLASSW {
                    style: CS_DROPSHADOW,
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance,
                    hCursor: unsafe { LoadCursorW(null_mut(), IDC_ARROW) },
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
        // SAFETY: The stable Box remains owned even if setup fails; Drop destroys the HWND.
        let window = unsafe {
            CreateWindowExW(
                EX_STYLE,
                class_name.as_ptr(),
                title.as_ptr(),
                STYLE,
                0,
                0,
                WIDTH,
                HEIGHT,
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
        // Standard Windows 11 menus use the small radius; Windows 10 rejects the attribute.
        let corners = DWMWCP_ROUNDSMALL;
        self.state.rounded.set(
            unsafe {
                DwmSetWindowAttribute(
                    window,
                    DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                    (&raw const corners).cast(),
                    size_of_val(&corners) as u32,
                )
            } >= 0,
        );
        self.state.palette.set(Palette::current(window));
        self.state.ready.set(true);
        self.state.layout()
    }

    /// Keep the popup open across saves; a late acknowledgement must not reveal a closed popup.
    pub(crate) fn set_gain(&mut self, gain: f32, save_failed: bool) {
        let next = self.state.draft.borrow_mut().set_gain(gain, save_failed);
        self.state.repaint();
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
        unsafe { SetFocus(window) };
        Ok(())
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.state.open.get()
    }
}

impl Drop for NativeGain {
    fn drop(&mut self) {
        self.state.open.set(false);
        // Destroy the window and its callbacks before dropping the boxed state or its font.
        if !self.state.window.get().is_null() {
            unsafe { DestroyWindow(self.state.window.get()) };
        }
    }
}
