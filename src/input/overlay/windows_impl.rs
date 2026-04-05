use std::mem::MaybeUninit;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

const OVERLAY_SIZE: i32 = 48;
const CORNER_RADIUS: i32 = 12;
const SCREEN_MARGIN: i32 = 20;
const WINDOW_ALPHA: u8 = 242;
const DRAG_THRESHOLD: i32 = 3;

static CLICKED: AtomicBool = AtomicBool::new(false);
static IS_RECORDING: AtomicBool = AtomicBool::new(false);
static WINDOW_CLASS: OnceLock<Result<(), String>> = OnceLock::new();

pub struct OverlayManager {
    hwnd: ffi::HWND,
}

impl OverlayManager {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        register_window_class()?;

        let (x, y) = overlay_origin()?;
        let state = Box::new(WindowState::default());
        let state_ptr = Box::into_raw(state);

        let hwnd = unsafe {
            // SAFETY: The class has been registered, the title/class pointers are
            // valid NUL-terminated UTF-16 buffers for the duration of the call,
            // and the lpParam carries ownership of a Box<WindowState>.
            ffi::CreateWindowExW(
                ffi::WS_EX_TOOLWINDOW
                    | ffi::WS_EX_TOPMOST
                    | ffi::WS_EX_LAYERED
                    | ffi::WS_EX_NOACTIVATE,
                wide_null("ViberWhisperOverlay").as_ptr(),
                wide_null("ViberWhisper Overlay").as_ptr(),
                ffi::WS_POPUP | ffi::WS_VISIBLE,
                x,
                y,
                OVERLAY_SIZE,
                OVERLAY_SIZE,
                ptr::null_mut(),
                ptr::null_mut(),
                ffi::GetModuleHandleW(ptr::null()),
                state_ptr.cast(),
            )
        };

        if hwnd.is_null() {
            unsafe {
                // SAFETY: state_ptr came from Box::into_raw above and window
                // creation failed, so ownership needs to be reclaimed here.
                drop(Box::from_raw(state_ptr));
            }
            return Err(last_os_error("CreateWindowExW failed").into());
        }

        unsafe {
            // SAFETY: hwnd is a valid layered popup window we just created.
            ffi::SetLayeredWindowAttributes(hwnd, 0, WINDOW_ALPHA, ffi::LWA_ALPHA);
            ffi::ShowWindow(hwnd, ffi::SW_SHOWNOACTIVATE);
            ffi::UpdateWindow(hwnd);
        }

        Ok(Self { hwnd })
    }

    pub fn set_recording(&mut self, recording: bool) {
        IS_RECORDING.store(recording, Ordering::Relaxed);

        unsafe {
            // SAFETY: hwnd belongs to this overlay window. Passing null RECT
            // invalidates the whole client area and requests repaint.
            ffi::InvalidateRect(self.hwnd, ptr::null(), 1);
        }
    }

    pub fn check_click(&self) -> bool {
        CLICKED.swap(false, Ordering::Relaxed)
    }

    pub fn update(&self) {
        unsafe {
            // SAFETY: Standard non-blocking Win32 message pump.
            let mut msg = MaybeUninit::<ffi::MSG>::zeroed();
            while ffi::PeekMessageW(msg.as_mut_ptr(), ptr::null_mut(), 0, 0, ffi::PM_REMOVE) != 0 {
                let msg = msg.assume_init();
                ffi::TranslateMessage(&msg);
                ffi::DispatchMessageW(&msg);
            }
        }
    }
}

impl Drop for OverlayManager {
    fn drop(&mut self) {
        if !self.hwnd.is_null() {
            unsafe {
                // SAFETY: hwnd belongs to this window. DestroyWindow is idempotent
                // from our ownership perspective because we clear the handle after.
                ffi::DestroyWindow(self.hwnd);
            }
            self.hwnd = ptr::null_mut();
        }
    }
}

#[derive(Default)]
struct WindowState {
    mouse_down: bool,
    drag_started: bool,
    press_screen: ffi::POINT,
    window_origin: ffi::POINT,
}

fn register_window_class() -> Result<(), Box<dyn std::error::Error>> {
    let result = WINDOW_CLASS.get_or_init(|| {
        let class_name = wide_null("ViberWhisperOverlay");
        let hinstance = unsafe {
            // SAFETY: Null module name requests the current process module handle.
            ffi::GetModuleHandleW(ptr::null())
        };

        let cursor = unsafe {
            // SAFETY: Loading the predefined arrow cursor with null instance is valid.
            ffi::LoadCursorW(ptr::null_mut(), ffi::IDC_ARROW)
        };

        let wnd_class = ffi::WNDCLASSW {
            style: ffi::CS_HREDRAW | ffi::CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: hinstance,
            lpszClassName: class_name.as_ptr(),
            hCursor: cursor,
            hbrBackground: ptr::null_mut(),
            cbClsExtra: 0,
            cbWndExtra: 0,
            lpszMenuName: ptr::null(),
            hIcon: ptr::null_mut(),
        };

        let atom = unsafe {
            // SAFETY: wnd_class points to a fully initialized class descriptor.
            ffi::RegisterClassW(&wnd_class)
        };

        if atom == 0 {
            let err = unsafe { ffi::GetLastError() };
            if err != ffi::ERROR_CLASS_ALREADY_EXISTS {
                return Err(last_os_error("RegisterClassW failed"));
            }
        }

        Ok(())
    });

    if let Err(err) = result {
        return Err(err.clone().into());
    }

    Ok(())
}

fn overlay_origin() -> Result<(i32, i32), Box<dyn std::error::Error>> {
    let mut work_area = ffi::RECT::default();
    let ok = unsafe {
        // SAFETY: work_area points to writable memory large enough for RECT.
        ffi::SystemParametersInfoW(
            ffi::SPI_GETWORKAREA,
            0,
            (&mut work_area as *mut ffi::RECT).cast(),
            0,
        )
    };

    if ok == 0 {
        return Err(last_os_error("SystemParametersInfoW(SPI_GETWORKAREA) failed").into());
    }

    Ok((
        work_area.right - OVERLAY_SIZE - SCREEN_MARGIN,
        work_area.bottom - OVERLAY_SIZE - SCREEN_MARGIN,
    ))
}

unsafe extern "system" fn window_proc(
    hwnd: ffi::HWND,
    msg: u32,
    wparam: ffi::WPARAM,
    lparam: ffi::LPARAM,
) -> ffi::LRESULT {
    match msg {
        ffi::WM_NCCREATE => {
            let create_struct = lparam as *const ffi::CREATESTRUCTW;
            if !create_struct.is_null() {
                let state_ptr = unsafe { (*create_struct).lpCreateParams as *mut WindowState };
                unsafe {
                    // SAFETY: During WM_NCCREATE the lpCreateParams is our Box pointer.
                    ffi::SetWindowLongPtrW(hwnd, ffi::GWLP_USERDATA, state_ptr as isize);
                }

                let region = unsafe {
                    // SAFETY: Creates a valid rounded region for the fixed overlay size.
                    ffi::CreateRoundRectRgn(
                        0,
                        0,
                        OVERLAY_SIZE + 1,
                        OVERLAY_SIZE + 1,
                        CORNER_RADIUS,
                        CORNER_RADIUS,
                    )
                };
                if !region.is_null() {
                    unsafe {
                        // SAFETY: The OS takes ownership of the region on success.
                        ffi::SetWindowRgn(hwnd, region, 1);
                    }
                }
            }
            1
        }
        ffi::WM_MOUSEACTIVATE => ffi::MA_NOACTIVATE as isize,
        ffi::WM_LBUTTONDOWN => {
            if let Some(state) = window_state_mut(hwnd) {
                state.mouse_down = true;
                state.drag_started = false;
                let cursor = current_cursor_pos();
                state.press_screen = cursor;
                state.window_origin = window_top_left(hwnd);
                unsafe {
                    // SAFETY: hwnd is our active overlay window.
                    ffi::SetCapture(hwnd);
                }
            }
            0
        }
        ffi::WM_MOUSEMOVE => {
            if let Some(state) = window_state_mut(hwnd)
                && state.mouse_down
            {
                let cursor = current_cursor_pos();
                let dx = cursor.x - state.press_screen.x;
                let dy = cursor.y - state.press_screen.y;

                if !state.drag_started && (dx.abs() >= DRAG_THRESHOLD || dy.abs() >= DRAG_THRESHOLD)
                {
                    state.drag_started = true;
                }

                if state.drag_started {
                    unsafe {
                        // SAFETY: hwnd is valid and remains topmost/no-activate while moving.
                        ffi::SetWindowPos(
                            hwnd,
                            ffi::HWND_TOPMOST,
                            state.window_origin.x + dx,
                            state.window_origin.y + dy,
                            0,
                            0,
                            ffi::SWP_NOSIZE | ffi::SWP_NOACTIVATE,
                        );
                    }
                }
            }
            0
        }
        ffi::WM_LBUTTONUP => {
            if let Some(state) = window_state_mut(hwnd)
                && state.mouse_down
            {
                state.mouse_down = false;
                unsafe {
                    // SAFETY: Releases capture obtained in WM_LBUTTONDOWN.
                    ffi::ReleaseCapture();
                }

                if !state.drag_started {
                    CLICKED.store(true, Ordering::Relaxed);
                }
            }
            0
        }
        ffi::WM_PAINT => {
            paint_overlay(hwnd);
            0
        }
        ffi::WM_SETTINGCHANGE | ffi::WM_THEMECHANGED => {
            unsafe {
                // SAFETY: hwnd is valid; invalidating the full client rect is enough
                // to repaint with the latest theme-derived colors.
                ffi::InvalidateRect(hwnd, ptr::null(), 1);
            }
            0
        }
        ffi::WM_ERASEBKGND => 1,
        ffi::WM_NCDESTROY => {
            let ptr =
                unsafe { ffi::GetWindowLongPtrW(hwnd, ffi::GWLP_USERDATA) as *mut WindowState };
            if !ptr.is_null() {
                unsafe {
                    // SAFETY: This pointer came from Box::into_raw in OverlayManager::new.
                    drop(Box::from_raw(ptr));
                    ffi::SetWindowLongPtrW(hwnd, ffi::GWLP_USERDATA, 0);
                }
            }
            unsafe {
                // SAFETY: Delegate final cleanup to the default proc.
                ffi::DefWindowProcW(hwnd, msg, wparam, lparam)
            }
        }
        _ => unsafe {
            // SAFETY: Unhandled messages are forwarded to the default proc.
            ffi::DefWindowProcW(hwnd, msg, wparam, lparam)
        },
    }
}

fn paint_overlay(hwnd: ffi::HWND) {
    unsafe {
        // SAFETY: Standard BeginPaint / EndPaint pair around all GDI drawing.
        let mut ps = MaybeUninit::<ffi::PAINTSTRUCT>::zeroed();
        let hdc = ffi::BeginPaint(hwnd, ps.as_mut_ptr());
        if hdc.is_null() {
            return;
        }

        let ps = ps.assume_init();
        let mut rect = ffi::RECT::default();
        ffi::GetClientRect(hwnd, &mut rect);

        let background = background_color();
        let bg_brush = ffi::CreateSolidBrush(background);
        let border_pen = ffi::CreatePen(ffi::PS_SOLID, 1, background);

        let old_brush = ffi::SelectObject(hdc, bg_brush.cast());
        let old_pen = ffi::SelectObject(hdc, border_pen.cast());

        ffi::RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            CORNER_RADIUS,
            CORNER_RADIUS,
        );

        ffi::SelectObject(hdc, old_brush);
        ffi::SelectObject(hdc, old_pen);
        ffi::DeleteObject(bg_brush.cast());
        ffi::DeleteObject(border_pen.cast());

        draw_mic_icon(hdc);
        ffi::EndPaint(hwnd, &ps);
    }
}

fn draw_mic_icon(hdc: ffi::HDC) {
    let icon_color = mic_icon_color(IS_RECORDING.load(Ordering::Relaxed));

    unsafe {
        // SAFETY: GDI object lifetimes are paired within this function.
        let brush = ffi::CreateSolidBrush(icon_color);
        let pen = ffi::CreatePen(ffi::PS_SOLID, 2, icon_color);
        let old_brush = ffi::SelectObject(hdc, brush.cast());
        let old_pen = ffi::SelectObject(hdc, pen.cast());

        ffi::SetBkMode(hdc, ffi::TRANSPARENT);

        ffi::RoundRect(hdc, 19, 15, 29, 31, 10, 10);
        ffi::Arc(hdc, 15, 13, 33, 33, 15, 24, 33, 24);
        ffi::MoveToEx(hdc, 24, 28, ptr::null_mut());
        ffi::LineTo(hdc, 24, 34);
        ffi::MoveToEx(hdc, 19, 34, ptr::null_mut());
        ffi::LineTo(hdc, 29, 34);

        ffi::SelectObject(hdc, old_brush);
        ffi::SelectObject(hdc, old_pen);
        ffi::DeleteObject(brush.cast());
        ffi::DeleteObject(pen.cast());
    }
}

fn background_color() -> u32 {
    if is_dark_mode() {
        rgb(51, 51, 51)
    } else {
        rgb(242, 242, 242)
    }
}

fn mic_icon_color(recording: bool) -> u32 {
    if recording {
        rgb(230, 51, 51)
    } else if is_dark_mode() {
        rgb(230, 230, 230)
    } else {
        rgb(77, 77, 77)
    }
}

fn is_dark_mode() -> bool {
    match read_apps_use_light_theme() {
        Some(light_mode) => !light_mode,
        None => {
            let color = unsafe {
                // SAFETY: Querying a system color is side-effect free.
                ffi::GetSysColor(ffi::COLOR_WINDOW)
            };
            perceived_luminance(color) < 128
        }
    }
}

fn read_apps_use_light_theme() -> Option<bool> {
    let mut value = 1u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let result = unsafe {
        // SAFETY: Registry path and value name are valid UTF-16 strings; value
        // points to writable storage for a DWORD.
        ffi::RegGetValueW(
            ffi::HKEY_CURRENT_USER,
            wide_null("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize").as_ptr(),
            wide_null("AppsUseLightTheme").as_ptr(),
            ffi::RRF_RT_REG_DWORD,
            ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };

    if result == 0 { Some(value != 0) } else { None }
}

fn perceived_luminance(color: u32) -> u8 {
    let r = (color & 0xFF) as u32;
    let g = ((color >> 8) & 0xFF) as u32;
    let b = ((color >> 16) & 0xFF) as u32;
    ((r * 299 + g * 587 + b * 114) / 1000) as u8
}

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}

fn current_cursor_pos() -> ffi::POINT {
    let mut point = ffi::POINT::default();
    unsafe {
        // SAFETY: point points to writable storage for the current cursor position.
        ffi::GetCursorPos(&mut point);
    }
    point
}

fn window_top_left(hwnd: ffi::HWND) -> ffi::POINT {
    let mut rect = ffi::RECT::default();
    unsafe {
        // SAFETY: rect points to writable storage for the window bounds.
        ffi::GetWindowRect(hwnd, &mut rect);
    }
    ffi::POINT {
        x: rect.left,
        y: rect.top,
    }
}

fn window_state_mut(hwnd: ffi::HWND) -> Option<&'static mut WindowState> {
    let ptr = unsafe {
        // SAFETY: Retrieves the Box<WindowState> pointer associated in WM_NCCREATE.
        ffi::GetWindowLongPtrW(hwnd, ffi::GWLP_USERDATA) as *mut WindowState
    };

    if ptr.is_null() {
        None
    } else {
        Some(unsafe {
            // SAFETY: The pointer remains valid until WM_NCDESTROY, where it is reclaimed.
            &mut *ptr
        })
    }
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_os_error(message: &str) -> String {
    format!("{message}: {}", std::io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_flags_default() {
        assert!(!CLICKED.load(Ordering::Relaxed));
        assert!(!IS_RECORDING.load(Ordering::Relaxed));
    }

    #[test]
    fn test_rgb_layout() {
        assert_eq!(rgb(1, 2, 3), 0x0003_0201);
    }
}

#[allow(non_snake_case)]
mod ffi {
    use std::ffi::c_void;

    pub type Bool = i32;
    pub type Dword = u32;
    pub type Uint = u32;
    pub type Wparam = usize;
    pub type Lparam = isize;
    pub type Lresult = isize;
    pub type LongPtr = isize;
    pub type Atom = u16;
    pub type Hwnd = *mut c_void;
    pub type Hinstance = *mut c_void;
    pub type Hicon = *mut c_void;
    pub type Hcursor = *mut c_void;
    pub type Hbrush = *mut c_void;
    pub type Hgdobj = *mut c_void;
    pub type Hdc = *mut c_void;
    pub type Hrgn = *mut c_void;
    pub type Hmenu = *mut c_void;
    pub type Lpcwstr = *const u16;

    pub use Atom as ATOM;
    pub use Bool as BOOL;
    pub use Dword as DWORD;
    pub use Hbrush as HBRUSH;
    pub use Hcursor as HCURSOR;
    pub use Hdc as HDC;
    pub use Hgdobj as HGDIOBJ;
    pub use Hicon as HICON;
    pub use Hinstance as HINSTANCE;
    pub use Hmenu as HMENU;
    pub use Hrgn as HRGN;
    pub use Hwnd as HWND;
    pub use LongPtr as LONG_PTR;
    pub use Lparam as LPARAM;
    pub use Lpcwstr as LPCWSTR;
    pub use Lresult as LRESULT;
    pub use Uint as UINT;
    pub use Wparam as WPARAM;

    pub const COLOR_WINDOW: i32 = 5;
    pub const CS_HREDRAW: u32 = 0x0002;
    pub const CS_VREDRAW: u32 = 0x0001;
    pub const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;
    pub const GWLP_USERDATA: i32 = -21;
    pub const IDC_ARROW: LPCWSTR = 32512usize as LPCWSTR;
    pub const LWA_ALPHA: u32 = 0x00000002;
    pub const MA_NOACTIVATE: i32 = 3;
    pub const PM_REMOVE: u32 = 0x0001;
    pub const PS_SOLID: i32 = 0;
    pub const RRF_RT_REG_DWORD: u32 = 0x00000018;
    pub const SPI_GETWORKAREA: u32 = 0x0030;
    pub const SW_SHOWNOACTIVATE: i32 = 4;
    pub const SWP_NOSIZE: u32 = 0x0001;
    pub const SWP_NOACTIVATE: u32 = 0x0010;
    pub const TRANSPARENT: i32 = 1;
    pub const WM_ERASEBKGND: u32 = 0x0014;
    pub const WM_LBUTTONDOWN: u32 = 0x0201;
    pub const WM_LBUTTONUP: u32 = 0x0202;
    pub const WM_MOUSEACTIVATE: u32 = 0x0021;
    pub const WM_MOUSEMOVE: u32 = 0x0200;
    pub const WM_NCCREATE: u32 = 0x0081;
    pub const WM_NCDESTROY: u32 = 0x0082;
    pub const WM_PAINT: u32 = 0x000F;
    pub const WM_SETTINGCHANGE: u32 = 0x001A;
    pub const WM_THEMECHANGED: u32 = 0x031A;
    pub const WS_EX_LAYERED: u32 = 0x00080000;
    pub const WS_EX_NOACTIVATE: u32 = 0x08000000;
    pub const WS_EX_TOOLWINDOW: u32 = 0x00000080;
    pub const WS_EX_TOPMOST: u32 = 0x00000008;
    pub const WS_POPUP: u32 = 0x80000000;
    pub const WS_VISIBLE: u32 = 0x10000000;

    pub const HKEY_CURRENT_USER: *mut c_void = 0x80000001usize as *mut c_void;
    pub const HWND_TOPMOST: HWND = -1isize as HWND;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct Rect {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Msg {
        pub hwnd: HWND,
        pub message: UINT,
        pub wParam: WPARAM,
        pub lParam: LPARAM,
        pub time: DWORD,
        pub pt: POINT,
        pub lPrivate: DWORD,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct PaintStruct {
        pub hdc: HDC,
        pub fErase: BOOL,
        pub rcPaint: RECT,
        pub fRestore: BOOL,
        pub fIncUpdate: BOOL,
        pub rgbReserved: [u8; 32],
    }

    #[repr(C)]
    pub struct CreateStructW {
        pub lpCreateParams: *mut c_void,
        pub hInstance: HINSTANCE,
        pub hMenu: HMENU,
        pub hwndParent: HWND,
        pub cy: i32,
        pub cx: i32,
        pub y: i32,
        pub x: i32,
        pub style: i32,
        pub lpszName: LPCWSTR,
        pub lpszClass: LPCWSTR,
        pub dwExStyle: DWORD,
    }

    pub type Wndproc = Option<unsafe extern "system" fn(HWND, UINT, WPARAM, LPARAM) -> LRESULT>;

    #[repr(C)]
    pub struct WndClassW {
        pub style: UINT,
        pub lpfnWndProc: WNDPROC,
        pub cbClsExtra: i32,
        pub cbWndExtra: i32,
        pub hInstance: HINSTANCE,
        pub hIcon: HICON,
        pub hCursor: HCURSOR,
        pub hbrBackground: HBRUSH,
        pub lpszMenuName: LPCWSTR,
        pub lpszClassName: LPCWSTR,
    }

    pub use CreateStructW as CREATESTRUCTW;
    pub use Msg as MSG;
    pub use PaintStruct as PAINTSTRUCT;
    pub use Point as POINT;
    pub use Rect as RECT;
    pub use WndClassW as WNDCLASSW;
    pub use Wndproc as WNDPROC;

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn BeginPaint(hwnd: HWND, lpPaint: *mut PAINTSTRUCT) -> HDC;
        pub fn CreateWindowExW(
            dwExStyle: DWORD,
            lpClassName: LPCWSTR,
            lpWindowName: LPCWSTR,
            dwStyle: DWORD,
            X: i32,
            Y: i32,
            nWidth: i32,
            nHeight: i32,
            hWndParent: HWND,
            hMenu: HMENU,
            hInstance: HINSTANCE,
            lpParam: *mut c_void,
        ) -> HWND;
        pub fn DefWindowProcW(hwnd: HWND, msg: UINT, wParam: WPARAM, lParam: LPARAM) -> LRESULT;
        pub fn DestroyWindow(hwnd: HWND) -> BOOL;
        pub fn DispatchMessageW(lpMsg: *const MSG) -> LRESULT;
        pub fn EndPaint(hwnd: HWND, lpPaint: *const PAINTSTRUCT) -> BOOL;
        pub fn GetClientRect(hwnd: HWND, lpRect: *mut RECT) -> BOOL;
        pub fn GetCursorPos(lpPoint: *mut POINT) -> BOOL;
        pub fn GetSysColor(nIndex: i32) -> DWORD;
        pub fn GetWindowLongPtrW(hwnd: HWND, nIndex: i32) -> LONG_PTR;
        pub fn GetWindowRect(hwnd: HWND, lpRect: *mut RECT) -> BOOL;
        pub fn InvalidateRect(hwnd: HWND, lpRect: *const RECT, bErase: BOOL) -> BOOL;
        pub fn LoadCursorW(hInstance: HINSTANCE, lpCursorName: LPCWSTR) -> HCURSOR;
        pub fn PeekMessageW(
            lpMsg: *mut MSG,
            hWnd: HWND,
            wMsgFilterMin: UINT,
            wMsgFilterMax: UINT,
            wRemoveMsg: UINT,
        ) -> BOOL;
        pub fn RegisterClassW(lpWndClass: *const WNDCLASSW) -> ATOM;
        pub fn ReleaseCapture() -> BOOL;
        pub fn SetCapture(hwnd: HWND) -> HWND;
        pub fn SetLayeredWindowAttributes(
            hwnd: HWND,
            crKey: DWORD,
            bAlpha: u8,
            dwFlags: DWORD,
        ) -> BOOL;
        pub fn SetWindowLongPtrW(hwnd: HWND, nIndex: i32, dwNewLong: LONG_PTR) -> LONG_PTR;
        pub fn SetWindowPos(
            hwnd: HWND,
            hwndInsertAfter: HWND,
            X: i32,
            Y: i32,
            cx: i32,
            cy: i32,
            uFlags: UINT,
        ) -> BOOL;
        pub fn SetWindowRgn(hwnd: HWND, hRgn: HRGN, bRedraw: BOOL) -> i32;
        pub fn ShowWindow(hwnd: HWND, nCmdShow: i32) -> BOOL;
        pub fn SystemParametersInfoW(
            uiAction: UINT,
            uiParam: UINT,
            pvParam: *mut c_void,
            fWinIni: UINT,
        ) -> BOOL;
        pub fn TranslateMessage(lpMsg: *const MSG) -> BOOL;
        pub fn UpdateWindow(hwnd: HWND) -> BOOL;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        pub fn Arc(
            hdc: HDC,
            left: i32,
            top: i32,
            right: i32,
            bottom: i32,
            xr1: i32,
            yr1: i32,
            xr2: i32,
            yr2: i32,
        ) -> BOOL;
        pub fn CreatePen(iStyle: i32, cWidth: i32, color: DWORD) -> HGDIOBJ;
        pub fn CreateRoundRectRgn(x1: i32, y1: i32, x2: i32, y2: i32, w: i32, h: i32) -> HRGN;
        pub fn CreateSolidBrush(color: DWORD) -> HBRUSH;
        pub fn DeleteObject(ho: HGDIOBJ) -> BOOL;
        pub fn LineTo(hdc: HDC, x: i32, y: i32) -> BOOL;
        pub fn MoveToEx(hdc: HDC, x: i32, y: i32, lppt: *mut POINT) -> BOOL;
        pub fn RoundRect(
            hdc: HDC,
            left: i32,
            top: i32,
            right: i32,
            bottom: i32,
            width: i32,
            height: i32,
        ) -> BOOL;
        pub fn SelectObject(hdc: HDC, h: HGDIOBJ) -> HGDIOBJ;
        pub fn SetBkMode(hdc: HDC, mode: i32) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetLastError() -> DWORD;
        pub fn GetModuleHandleW(lpModuleName: LPCWSTR) -> HINSTANCE;
    }

    #[link(name = "advapi32")]
    unsafe extern "system" {
        pub fn RegGetValueW(
            hkey: *mut c_void,
            lpSubKey: LPCWSTR,
            lpValue: LPCWSTR,
            dwFlags: DWORD,
            pdwType: *mut DWORD,
            pvData: *mut c_void,
            pcbData: *mut DWORD,
        ) -> i32;
    }
}
