use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, extern_methods};
use objc2_app_kit::{
    NSApp, NSAppearance, NSAppearanceNameAccessibilityHighContrastDarkAqua,
    NSAppearanceNameDarkAqua, NSAppearanceNameVibrantDark, NSApplication,
    NSApplicationActivationPolicy, NSBackingStoreType, NSBezierPath, NSColor, NSEvent,
    NSEventMask, NSScreen, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};
use std::sync::atomic::{AtomicBool, Ordering};

const OVERLAY_SIZE: f64 = 48.0;
const CORNER_RADIUS: f64 = 12.0;
const SCREEN_MARGIN: f64 = 20.0;

static CLICKED: AtomicBool = AtomicBool::new(false);
static IS_RECORDING: AtomicBool = AtomicBool::new(false);

pub struct OverlayManager {
    window: Retained<NSWindow>,
    content_view: Retained<OverlayView>,
}

impl OverlayManager {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "OverlayManager::new must run on the main thread".to_string())?;

        let app = NSApp(mtm);
        let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

        let screen = NSScreen::mainScreen(mtm)
            .ok_or_else(|| "No main screen available for overlay window".to_string())?;
        let screen_frame = screen.frame();

        let x = screen_frame.origin.x + screen_frame.size.width - OVERLAY_SIZE - SCREEN_MARGIN;
        let y = screen_frame.origin.y + SCREEN_MARGIN;
        let window_rect = NSRect::new(NSPoint::new(x, y), NSSize::new(OVERLAY_SIZE, OVERLAY_SIZE));

        let window = unsafe {
            // SAFETY: This runs on the main thread, allocates a fresh NSWindow,
            // and passes valid AppKit initialization parameters.
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                window_rect,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };

        window.setLevel(25);
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(true);
        window.setMovableByWindowBackground(true);
        window.setIgnoresMouseEvents(false);
        window.setAlphaValue(0.95);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary,
        );

        let content_view = create_overlay_view(window_rect.size, mtm);
        window.setContentView(Some(&content_view));
        window.makeKeyAndOrderFront(None::<&AnyObject>);
        window.resignKeyWindow();

        Ok(Self {
            window,
            content_view,
        })
    }

    pub fn set_recording(&mut self, recording: bool) {
        IS_RECORDING.store(recording, Ordering::Relaxed);
        self.content_view.setNeedsDisplay(true);
    }

    pub fn check_click(&self) -> bool {
        CLICKED.swap(false, Ordering::Relaxed)
    }

    pub fn update(&self) {
        let mtm = MainThreadMarker::from(&*self.window);
        let app: Retained<NSApplication> = NSApp(mtm);

        loop {
            let event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::all(),
                None::<&NSDate>,
                // SAFETY: NSDefaultRunLoopMode is an AppKit-provided immutable
                // global run loop mode constant.
                unsafe { NSDefaultRunLoopMode },
                true,
            );

            let Some(event) = event else {
                break;
            };

            app.sendEvent(&event);
        }
    }
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "VWOverlayView"]
    struct OverlayView;

    impl OverlayView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            let bounds = self.bounds();
            let recording = IS_RECORDING.load(Ordering::Relaxed);

            let bg_color = background_color();
            let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                bounds,
                CORNER_RADIUS,
                CORNER_RADIUS,
            );

            bg_color.setFill();
            path.fill();

            draw_mic_icon(bounds, recording);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            CLICKED.store(true, Ordering::Relaxed);
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            true
        }
    }
);

impl OverlayView {
    extern_methods!(
        #[unsafe(method(initWithFrame:))]
        #[unsafe(method_family = init)]
        fn init_with_frame(this: Allocated<Self>, frame_rect: NSRect) -> Retained<Self>;
    );
}

fn create_overlay_view(size: NSSize, mtm: MainThreadMarker) -> Retained<OverlayView> {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
    OverlayView::init_with_frame(OverlayView::alloc(mtm), frame)
}

fn is_dark_mode() -> bool {
    let appearance = NSAppearance::currentDrawingAppearance();
    let name: Retained<NSString> = appearance.name();

    unsafe {
        // SAFETY: These NSAppearanceName values are immutable AppKit-provided
        // global constants used only for comparison.
        name.isEqualToString(NSAppearanceNameDarkAqua)
            || name.isEqualToString(NSAppearanceNameVibrantDark)
            || name.isEqualToString(NSAppearanceNameAccessibilityHighContrastDarkAqua)
    }
}

fn background_color() -> Retained<NSColor> {
    if is_dark_mode() {
        NSColor::colorWithRed_green_blue_alpha(0.2, 0.2, 0.2, 0.9)
    } else {
        NSColor::colorWithRed_green_blue_alpha(0.95, 0.95, 0.95, 0.9)
    }
}

fn mic_icon_color(recording: bool) -> Retained<NSColor> {
    if recording {
        NSColor::colorWithRed_green_blue_alpha(0.9, 0.2, 0.2, 1.0)
    } else if is_dark_mode() {
        NSColor::colorWithRed_green_blue_alpha(0.9, 0.9, 0.9, 1.0)
    } else {
        NSColor::colorWithRed_green_blue_alpha(0.3, 0.3, 0.3, 1.0)
    }
}

fn draw_mic_icon(bounds: NSRect, recording: bool) {
    let icon_color = mic_icon_color(recording);
    icon_color.setFill();
    icon_color.setStroke();

    let cx = bounds.origin.x + bounds.size.width / 2.0;
    let cy = bounds.origin.y + bounds.size.height / 2.0;
    let scale = bounds.size.width / 48.0;

    let mic_width = 10.0 * scale;
    let mic_height = 16.0 * scale;
    let mic_rect = NSRect::new(
        NSPoint::new(cx - mic_width / 2.0, cy - 1.0 * scale),
        NSSize::new(mic_width, mic_height),
    );
    let mic_path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
        mic_rect,
        mic_width / 2.0,
        mic_width / 2.0,
    );
    mic_path.fill();

    let arc_path = NSBezierPath::bezierPath();
    arc_path.setLineWidth(2.0 * scale);
    arc_path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle(
        NSPoint::new(cx, cy + 6.0 * scale),
        9.0 * scale,
        210.0,
        330.0,
    );
    arc_path.stroke();

    let stand_path = NSBezierPath::bezierPath();
    stand_path.setLineWidth(2.0 * scale);
    stand_path.moveToPoint(NSPoint::new(cx, cy - 3.0 * scale));
    stand_path.lineToPoint(NSPoint::new(cx, cy - 8.0 * scale));
    stand_path.stroke();

    let base_path = NSBezierPath::bezierPath();
    base_path.setLineWidth(2.0 * scale);
    base_path.moveToPoint(NSPoint::new(cx - 5.0 * scale, cy - 8.0 * scale));
    base_path.lineToPoint(NSPoint::new(cx + 5.0 * scale, cy - 8.0 * scale));
    base_path.stroke();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_flags_default() {
        assert!(!CLICKED.load(Ordering::Relaxed));
        assert!(!IS_RECORDING.load(Ordering::Relaxed));
    }
}
