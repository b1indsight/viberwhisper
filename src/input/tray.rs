use std::time::{Duration, Instant};

#[cfg(not(test))]
use tracing::warn;
#[cfg(not(test))]
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
#[cfg(not(test))]
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const MIN_DEBOUNCE_WINDOW: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, allow(dead_code))]
pub enum TrayAction {
    ToggleRecording,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, allow(dead_code))]
enum ClickPhase {
    LeftUp,
    LeftDouble,
    Other,
}

struct ClickDebounce {
    window: Duration,
    last_accepted: Option<Instant>,
    suppress_next_up_until: Option<Instant>,
}

impl ClickDebounce {
    fn new(window: Duration) -> Self {
        Self {
            window,
            last_accepted: None,
            suppress_next_up_until: None,
        }
    }

    fn accept(&mut self, phase: ClickPhase, now: Instant) -> bool {
        match phase {
            ClickPhase::LeftDouble => {
                self.suppress_next_up_until = Some(now + self.window);
                false
            }
            ClickPhase::LeftUp => {
                if let Some(deadline) = self.suppress_next_up_until.take()
                    && now < deadline
                {
                    return false;
                }
                if let Some(last) = self.last_accepted
                    && now.duration_since(last) < self.window
                {
                    return false;
                }
                self.last_accepted = Some(now);
                true
            }
            ClickPhase::Other => false,
        }
    }
}

fn effective_debounce_window(platform_interval: Option<Duration>) -> Duration {
    platform_interval
        .unwrap_or(MIN_DEBOUNCE_WINDOW)
        .max(MIN_DEBOUNCE_WINDOW)
}

#[cfg(not(test))]
pub struct TrayManager {
    tray_icon: TrayIcon,
    icon_idle: Icon,
    icon_recording: Icon,
    status_item: MenuItem,
    exit_item_id: tray_icon::menu::MenuId,
    click_debounce: ClickDebounce,
}

// Keep test builds away from the native tray backend on Windows.
// The real `tray_icon` path is exercised in app runs, while tests only need
// a lightweight stand-in so CI can validate higher-level logic safely.
#[cfg(test)]
pub struct TrayManager;

#[cfg(not(test))]
impl TrayManager {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        prepare_platform_application()?;
        let icon_idle = create_icon(128, 128, 128, 255)?;
        let icon_recording = create_icon(220, 50, 50, 255)?;

        let menu = Menu::new();
        let title_item = MenuItem::new("ViberWhisper", false, None);
        let status_item = MenuItem::new("状态：空闲", false, None);
        let separator = PredefinedMenuItem::separator();
        let exit_item = MenuItem::new("退出", true, None);
        let exit_id = exit_item.id().clone();

        menu.append(&title_item)?;
        menu.append(&status_item)?;
        menu.append(&separator)?;
        menu.append(&exit_item)?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("ViberWhisper - 空闲")
            .with_icon(icon_idle.clone())
            .build()?;

        Ok(TrayManager {
            tray_icon,
            icon_idle,
            icon_recording,
            status_item,
            exit_item_id: exit_id,
            click_debounce: ClickDebounce::new(effective_debounce_window(
                platform_double_click_interval(),
            )),
        })
    }

    pub fn set_recording(&mut self, recording: bool) {
        let icon = if recording {
            &self.icon_recording
        } else {
            &self.icon_idle
        };
        let tooltip = if recording {
            "ViberWhisper - 录音中"
        } else {
            "ViberWhisper - 空闲"
        };

        self.status_item.set_text(if recording {
            "状态：录音中"
        } else {
            "状态：空闲"
        });
        if let Err(err) = self.tray_icon.set_icon(Some(icon.clone())) {
            warn!(error = ?err, "failed to update tray icon");
        }
        if let Err(err) = self.tray_icon.set_tooltip(Some(tooltip)) {
            warn!(error = ?err, "failed to update tray tooltip");
        }
    }

    pub fn check_action(&mut self) -> Option<TrayAction> {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.exit_item_id {
                return Some(TrayAction::Exit);
            }
        }

        let now = Instant::now();
        let mut toggle = false;
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if event.id() != self.tray_icon.id() {
                continue;
            }
            let phase = match event {
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } => ClickPhase::LeftUp,
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                } => ClickPhase::LeftDouble,
                _ => ClickPhase::Other,
            };
            toggle |= self.click_debounce.accept(phase, now);
        }

        toggle.then_some(TrayAction::ToggleRecording)
    }

    pub fn update(&self) {
        pump_platform_events();
    }
}

#[cfg(all(target_os = "macos", not(test)))]
fn prepare_platform_application() -> Result<(), Box<dyn std::error::Error>> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApp, NSApplicationActivationPolicy};

    let mtm = MainThreadMarker::new().ok_or("tray must be created on the main thread")?;
    let _ = NSApp(mtm).setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    Ok(())
}

#[cfg(all(not(target_os = "macos"), not(test)))]
fn prepare_platform_application() -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

#[cfg(test)]
impl TrayManager {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(TrayManager)
    }

    pub fn set_recording(&mut self, _recording: bool) {}

    pub fn check_action(&mut self) -> Option<TrayAction> {
        None
    }

    pub fn update(&self) {}
}

#[cfg(all(target_os = "macos", not(test)))]
fn platform_double_click_interval() -> Option<Duration> {
    use objc2_app_kit::NSEvent;

    let seconds = NSEvent::doubleClickInterval();
    seconds
        .is_finite()
        .then(|| Duration::from_secs_f64(seconds))
}

#[cfg(all(target_os = "windows", not(test)))]
fn platform_double_click_interval() -> Option<Duration> {
    Some(Duration::from_millis(
        unsafe { windows_event_loop::GetDoubleClickTime() } as u64,
    ))
}

#[cfg(all(not(any(target_os = "macos", target_os = "windows")), not(test)))]
fn platform_double_click_interval() -> Option<Duration> {
    None
}

#[cfg(all(target_os = "macos", not(test)))]
fn pump_platform_events() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApp, NSEventMask};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode};

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApp(mtm);
    loop {
        let event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            NSEventMask::all(),
            None::<&NSDate>,
            unsafe { NSDefaultRunLoopMode },
            true,
        );
        let Some(event) = event else { break };
        app.sendEvent(&event);
    }
}

#[cfg(all(target_os = "windows", not(test)))]
fn pump_platform_events() {
    use std::mem::MaybeUninit;
    use std::ptr;

    unsafe {
        let mut msg = MaybeUninit::<windows_event_loop::MSG>::zeroed();
        while windows_event_loop::PeekMessageW(
            msg.as_mut_ptr(),
            ptr::null_mut(),
            0,
            0,
            windows_event_loop::PM_REMOVE,
        ) != 0
        {
            let msg = msg.assume_init();
            windows_event_loop::TranslateMessage(&msg);
            windows_event_loop::DispatchMessageW(&msg);
        }
    }
}

#[cfg(all(not(any(target_os = "macos", target_os = "windows")), not(test)))]
fn pump_platform_events() {}

#[cfg(all(target_os = "windows", not(test)))]
#[allow(non_snake_case, clippy::upper_case_acronyms)]
mod windows_event_loop {
    use std::ffi::c_void;

    pub type BOOL = i32;
    pub type HWND = *mut c_void;
    pub type UINT = u32;
    pub type WPARAM = usize;
    pub type LPARAM = isize;
    pub type LRESULT = isize;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct POINT {
        pub x: i32,
        pub y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct MSG {
        pub hwnd: HWND,
        pub message: UINT,
        pub wParam: WPARAM,
        pub lParam: LPARAM,
        pub time: u32,
        pub pt: POINT,
        pub lPrivate: u32,
    }

    pub const PM_REMOVE: UINT = 0x0001;

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn DispatchMessageW(msg: *const MSG) -> LRESULT;
        pub fn GetDoubleClickTime() -> UINT;
        pub fn PeekMessageW(
            msg: *mut MSG,
            hwnd: HWND,
            min_filter: UINT,
            max_filter: UINT,
            remove: UINT,
        ) -> BOOL;
        pub fn TranslateMessage(msg: *const MSG) -> BOOL;
    }
}

// Pure RGBA generator shared by tests so they can verify icon data without
// constructing a platform tray icon handle.
fn build_icon_rgba(r: u8, g: u8, b: u8, a: u8) -> (Vec<u8>, u32) {
    let size = 32u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let center = size as f32 / 2.0;
    let radius = center - 2.0;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();

            let idx = ((y * size + x) * 4) as usize;
            if dist <= radius {
                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = a;
            }
        }
    }

    (rgba, size)
}

#[cfg(not(test))]
fn create_icon(r: u8, g: u8, b: u8, a: u8) -> Result<Icon, tray_icon::BadIcon> {
    let (rgba, size) = build_icon_rgba(r, g, b, a);
    Icon::from_rgba(rgba, size, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_icon_rgba() {
        let (rgba, size) = build_icon_rgba(128, 128, 128, 255);
        assert_eq!(size, 32);
        assert_eq!(rgba.len(), (size * size * 4) as usize);
        assert_eq!(rgba[3], 0);
        let center = ((size / 2 * size + size / 2) * 4) as usize;
        assert_eq!(rgba[center], 128);
        assert_eq!(rgba[center + 1], 128);
        assert_eq!(rgba[center + 2], 128);
        assert_eq!(rgba[center + 3], 255);
    }

    #[test]
    fn effective_window_uses_platform_interval_with_300ms_floor() {
        assert_eq!(
            effective_debounce_window(Some(Duration::from_millis(200))),
            Duration::from_millis(300)
        );
        assert_eq!(
            effective_debounce_window(Some(Duration::from_millis(500))),
            Duration::from_millis(500)
        );
        assert_eq!(effective_debounce_window(None), Duration::from_millis(300));
    }

    #[test]
    fn debounce_accepts_boundary_and_ignored_click_does_not_extend_window() {
        let base = Instant::now();
        let mut debounce = ClickDebounce::new(Duration::from_millis(300));

        assert!(debounce.accept(ClickPhase::LeftUp, base));
        assert!(!debounce.accept(ClickPhase::LeftUp, base + Duration::from_millis(200)));
        assert!(debounce.accept(ClickPhase::LeftUp, base + Duration::from_millis(300)));
    }

    #[test]
    fn native_double_click_suppresses_only_the_trailing_left_up() {
        let base = Instant::now();
        let mut debounce = ClickDebounce::new(Duration::from_millis(300));

        assert!(debounce.accept(ClickPhase::LeftUp, base));
        assert!(!debounce.accept(ClickPhase::LeftDouble, base + Duration::from_millis(400)));
        assert!(!debounce.accept(ClickPhase::LeftUp, base + Duration::from_millis(401)));
        assert!(debounce.accept(ClickPhase::LeftUp, base + Duration::from_millis(402)));
    }
}
