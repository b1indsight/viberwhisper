#![allow(deprecated)]

use cocoa::appkit::{
    NSApp, NSBackingStoreBuffered, NSColor, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use cocoa::base::{NO, YES, id, nil};
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
use objc::declare::ClassDecl;
use objc::runtime::{BOOL, Class, Object, Sel};
use objc::{class, msg_send, sel, sel_impl};
use std::sync::atomic::{AtomicBool, Ordering};

const OVERLAY_SIZE: f64 = 48.0;
const CORNER_RADIUS: f64 = 12.0;
const SCREEN_MARGIN: f64 = 20.0;

static CLICKED: AtomicBool = AtomicBool::new(false);
static IS_RECORDING: AtomicBool = AtomicBool::new(false);

pub struct OverlayManager {
    _window: id,
    content_view: id,
}

unsafe impl Send for OverlayManager {}

impl OverlayManager {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        unsafe {
            let _pool = NSAutoreleasePool::new(nil);

            let app = NSApp();
            let _: () = msg_send![app, setActivationPolicy: 1i64];

            let screen: id = msg_send![class!(NSScreen), mainScreen];
            let screen_frame: NSRect = msg_send![screen, frame];

            let x = screen_frame.origin.x + screen_frame.size.width - OVERLAY_SIZE - SCREEN_MARGIN;
            let y = screen_frame.origin.y + SCREEN_MARGIN;

            let window_rect =
                NSRect::new(NSPoint::new(x, y), NSSize::new(OVERLAY_SIZE, OVERLAY_SIZE));

            let window = NSWindow::alloc(nil).initWithContentRect_styleMask_backing_defer_(
                window_rect,
                NSWindowStyleMask::NSBorderlessWindowMask,
                NSBackingStoreBuffered,
                NO,
            );

            window.setLevel_(25);
            let _: () = msg_send![window, setOpaque: NO];
            window.setBackgroundColor_(NSColor::clearColor(nil));
            let _: () = msg_send![window, setHasShadow: YES];
            let _: () = msg_send![window, setMovableByWindowBackground: YES];
            let _: () = msg_send![window, setIgnoresMouseEvents: NO];
            let _: () = msg_send![window, setAlphaValue: 0.95f64];

            window.setCollectionBehavior_(
                NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
                    | NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary,
            );

            let content_view = create_overlay_view(window_rect.size);
            window.setContentView_(content_view);

            window.makeKeyAndOrderFront_(nil);
            let _: () = msg_send![window, resignKeyWindow];

            Ok(OverlayManager {
                _window: window,
                content_view,
            })
        }
    }

    pub fn set_recording(&mut self, recording: bool) {
        IS_RECORDING.store(recording, Ordering::Relaxed);
        unsafe {
            let _: () = msg_send![self.content_view, setNeedsDisplay: YES];
        }
    }

    pub fn check_click(&self) -> bool {
        CLICKED.swap(false, Ordering::Relaxed)
    }

    pub fn update(&self) {
        unsafe {
            let app = NSApp();
            loop {
                let event: id = msg_send![app,
                    nextEventMatchingMask: u64::MAX
                    untilDate: nil
                    inMode: NSString::alloc(nil).init_str("kCFRunLoopDefaultMode")
                    dequeue: YES
                ];
                if event == nil {
                    break;
                }
                let _: () = msg_send![app, sendEvent: event];
            }
        }
    }
}

fn create_overlay_view(size: NSSize) -> id {
    unsafe {
        let superclass = Class::get("NSView").unwrap();

        if let Some(cls) = Class::get("VWOverlayView") {
            let view: id = msg_send![cls, alloc];
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
            let view: id = msg_send![view, initWithFrame: frame];
            return view;
        }

        let mut decl = ClassDecl::new("VWOverlayView", superclass).unwrap();

        extern "C" fn draw_rect(this: &Object, _sel: Sel, _dirty_rect: NSRect) {
            unsafe {
                let bounds: NSRect = msg_send![this, bounds];
                let recording = IS_RECORDING.load(Ordering::Relaxed);

                let bg_color = if is_dark_mode() {
                    NSColor::colorWithRed_green_blue_alpha_(nil, 0.2, 0.2, 0.2, 0.9)
                } else {
                    NSColor::colorWithRed_green_blue_alpha_(nil, 0.95, 0.95, 0.95, 0.9)
                };

                let path: id = msg_send![class!(NSBezierPath),
                    bezierPathWithRoundedRect: bounds
                    xRadius: CORNER_RADIUS
                    yRadius: CORNER_RADIUS
                ];

                let _: () = msg_send![bg_color, setFill];
                let _: () = msg_send![path, fill];

                draw_mic_icon(bounds, recording);
            }
        }

        extern "C" fn mouse_down(_this: &Object, _sel: Sel, _event: id) {
            CLICKED.store(true, Ordering::Relaxed);
        }

        extern "C" fn accepts_first_mouse(_this: &Object, _sel: Sel, _event: id) -> BOOL {
            YES
        }

        decl.add_method(
            sel!(drawRect:),
            draw_rect as extern "C" fn(&Object, Sel, NSRect),
        );
        decl.add_method(
            sel!(mouseDown:),
            mouse_down as extern "C" fn(&Object, Sel, id),
        );
        decl.add_method(
            sel!(acceptsFirstMouse:),
            accepts_first_mouse as extern "C" fn(&Object, Sel, id) -> BOOL,
        );

        let cls = decl.register();

        let view: id = msg_send![cls, alloc];
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), size);
        let view: id = msg_send![view, initWithFrame: frame];
        view
    }
}

fn is_dark_mode() -> bool {
    unsafe {
        let app = NSApp();
        let appearance: id = msg_send![app, effectiveAppearance];
        if appearance == nil {
            return false;
        }
        let name: id = msg_send![appearance, name];
        if name == nil {
            return false;
        }
        let dark_name = NSString::alloc(nil).init_str("NSAppearanceNameDarkAqua");
        let contains: BOOL = msg_send![name, containsString: dark_name];
        contains == YES
    }
}

fn draw_mic_icon(bounds: NSRect, recording: bool) {
    unsafe {
        let icon_color = if recording {
            NSColor::colorWithRed_green_blue_alpha_(nil, 0.9, 0.2, 0.2, 1.0)
        } else if is_dark_mode() {
            NSColor::colorWithRed_green_blue_alpha_(nil, 0.9, 0.9, 0.9, 1.0)
        } else {
            NSColor::colorWithRed_green_blue_alpha_(nil, 0.3, 0.3, 0.3, 1.0)
        };
        let _: () = msg_send![icon_color, setFill];
        let _: () = msg_send![icon_color, setStroke];

        let cx = bounds.origin.x + bounds.size.width / 2.0;
        let cy = bounds.origin.y + bounds.size.height / 2.0;
        let scale = bounds.size.width / 48.0;

        // Microphone body (rounded rectangle)
        let mic_width = 10.0 * scale;
        let mic_height = 16.0 * scale;
        let mic_rect = NSRect::new(
            NSPoint::new(cx - mic_width / 2.0, cy - 1.0 * scale),
            NSSize::new(mic_width, mic_height),
        );
        let mic_path: id = msg_send![class!(NSBezierPath),
            bezierPathWithRoundedRect: mic_rect
            xRadius: mic_width / 2.0
            yRadius: mic_width / 2.0
        ];
        let _: () = msg_send![mic_path, fill];

        // Microphone arc (holder curve)
        let arc_path: id = msg_send![class!(NSBezierPath), bezierPath];
        let _: () = msg_send![arc_path, setLineWidth: 2.0 * scale];
        let _: () = msg_send![arc_path,
            appendBezierPathWithArcWithCenter: NSPoint::new(cx, cy + 6.0 * scale)
            radius: 9.0 * scale
            startAngle: 210.0f64
            endAngle: 330.0f64
        ];
        let _: () = msg_send![arc_path, stroke];

        // Stand (vertical line)
        let stand_path: id = msg_send![class!(NSBezierPath), bezierPath];
        let _: () = msg_send![stand_path, setLineWidth: 2.0 * scale];
        let _: () = msg_send![stand_path, moveToPoint: NSPoint::new(cx, cy - 3.0 * scale)];
        let _: () = msg_send![stand_path, lineToPoint: NSPoint::new(cx, cy - 8.0 * scale)];
        let _: () = msg_send![stand_path, stroke];

        // Base
        let base_path: id = msg_send![class!(NSBezierPath), bezierPath];
        let _: () = msg_send![base_path, setLineWidth: 2.0 * scale];
        let _: () = msg_send![
            base_path,
            moveToPoint: NSPoint::new(cx - 5.0 * scale, cy - 8.0 * scale)
        ];
        let _: () = msg_send![
            base_path,
            lineToPoint: NSPoint::new(cx + 5.0 * scale, cy - 8.0 * scale)
        ];
        let _: () = msg_send![base_path, stroke];
    }
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
