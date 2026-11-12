//! An AppKit slider hosted by the existing tray menu, without a separate settings window.

use std::cell::{Cell, RefCell};

use anyhow::{Result, anyhow};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DeclaredClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSEvent, NSFont, NSMenu, NSMenuDelegate, NSSlider, NSTextField, NSView};
use objc2_foundation::{NSObjectProtocol, NSRect, NSString};
use tray_icon::menu::{ContextMenu, Menu, MenuItem};

use crate::platform::gain::{GainDraft, GainNotify};

struct ViewState {
    edit: RefCell<GainDraft>,
    notify: GainNotify,
    slider: Retained<GainSlider>,
    value: Retained<NSTextField>,
    notice: Retained<NSTextField>,
    visible: Cell<bool>,
}

define_class!(
    #[unsafe(super = NSSlider)]
    #[thread_kind = MainThreadOnly]
    struct GainSlider;

    unsafe impl NSObjectProtocol for GainSlider {}

    impl GainSlider {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if let Some(window) = self.window() {
                window.makeFirstResponder(Some(self));
            }
            // SAFETY: Keep AppKit's native thumb tracking after assigning keyboard focus.
            unsafe { let _: () = msg_send![super(self), mouseDown: event]; }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if let Some(target) = self.target()
                && let Some(view) = target.downcast_ref::<GainView>()
                && view.handle_key(event)
            {
                return;
            }
            // SAFETY: Forward an unhandled event to the actual NSSlider superclass.
            unsafe { let _: () = msg_send![super(self), keyDown: event]; }
        }
    }
);

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ViewState]
    struct GainView;

    unsafe impl NSObjectProtocol for GainView {}

    unsafe impl NSMenuDelegate for GainView {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _: &NSMenu) {
            self.ivars().visible.set(true);
            self.refresh();
        }

        #[unsafe(method(menuDidClose:))]
        fn menu_did_close(&self, _: &NSMenu) {
            self.ivars().visible.set(false);
            self.submit();
        }
    }

    impl GainView {
        #[unsafe(method(sliderChanged:))]
        fn slider_changed(&self, _: &AnyObject) {
            let position = self.ivars().slider.doubleValue().round() as i32;
            self.ivars().edit.borrow_mut().change(position);
            self.refresh();
            self.submit();
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool { true }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if !self.handle_key(event) {
                // SAFETY: Unhandled keys retain the normal NSView responder-chain behavior.
                unsafe { let _: () = msg_send![super(self), keyDown: event]; }
            }
        }
    }
);

impl GainView {
    fn refresh(&self) {
        let edit = self.ivars().edit.borrow();
        self.ivars()
            .slider
            .setDoubleValue(f64::from(edit.position()));
        self.ivars()
            .value
            .setStringValue(&NSString::from_str(&format!(
                "音频增益  {}",
                edit.value_label()
            )));
        self.ivars()
            .notice
            .setStringValue(&NSString::from_str(edit.notice()));
    }

    fn submit(&self) {
        let gain = self.ivars().edit.borrow_mut().submit();
        if let Some(gain) = gain {
            (self.ivars().notify)(gain);
        }
    }

    fn close_menu(&self) {
        if let Some(item) = self.enclosingMenuItem()
            // SAFETY: This live item was inserted into the retained tray menu.
            && let Some(menu) = unsafe { item.menu() }
        {
            menu.cancelTracking();
        }
    }

    fn handle_key(&self, event: &NSEvent) -> bool {
        match event.keyCode() {
            53 | 36 | 76 => self.close_menu(),
            123 | 124 | 125 | 126 | 115 | 119 => {
                let mut edit = self.ivars().edit.borrow_mut();
                let position = match event.keyCode() {
                    123 | 125 => edit.position() - 1,
                    124 | 126 => edit.position() + 1,
                    115 => 0,
                    _ => 200,
                };
                edit.change(position);
                drop(edit);
                self.refresh();
                self.submit();
            }
            _ => return false,
        }
        true
    }
}

pub(crate) struct NativeGain {
    view: Retained<GainView>,
    menu: Retained<NSMenu>,
}

impl NativeGain {
    pub(crate) fn new(menu: &Menu, item: &MenuItem, notify: GainNotify) -> Result<Self> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| anyhow!("gain menu must be created on the main thread"))?;
        let index = menu
            .items()
            .iter()
            .position(|entry| entry.id() == item.id())
            .ok_or_else(|| anyhow!("gain item is not attached to the tray menu"))?;
        // SAFETY: muda exposes this live NSMenu pointer; retaining bridges the Objective-C
        // runtime object without mixing Rust objects from muda's older objc2 dependency.
        let native = unsafe { Retained::retain(menu.ns_menu().cast::<NSMenu>()) }
            .ok_or_else(|| anyhow!("tray menu has no native menu"))?;
        let native_item = native
            .itemAtIndex(index as isize)
            .ok_or_else(|| anyhow!("gain menu item is missing"))?;
        let slider: Retained<GainSlider> = unsafe {
            msg_send![super(GainSlider::alloc(mtm).set_ivars(())), initWithFrame: NSRect::new(
                objc2_foundation::NSPoint::new(18.0, 44.0),
                objc2_foundation::NSSize::new(276.0, 24.0)
            )]
        };
        slider.setMinValue(0.0);
        slider.setMaxValue(200.0);
        // Native tracking emits one action on mouse release, not a config write per pixel.
        slider.setContinuous(false);
        let value = label(
            mtm,
            "",
            NSRect::new(
                objc2_foundation::NSPoint::new(18.0, 76.0),
                objc2_foundation::NSSize::new(276.0, 22.0),
            ),
        );
        let notice = label(
            mtm,
            "",
            NSRect::new(
                objc2_foundation::NSPoint::new(18.0, 4.0),
                objc2_foundation::NSSize::new(276.0, 20.0),
            ),
        );
        notice.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        let view: Retained<GainView> = unsafe {
            msg_send![super(GainView::alloc(mtm).set_ivars(ViewState {
                edit: RefCell::new(GainDraft::new(1.0)), notify,
                slider: slider.clone(), value: value.clone(), notice: notice.clone(),
                visible: Cell::new(false),
            })), initWithFrame: rect(0.0, 0.0, 312.0, 106.0)]
        };
        view.addSubview(&value);
        view.addSubview(&slider);
        view.addSubview(&notice);
        for (text, x) in [("−10 dB", 18.0), ("0", 151.0), ("+10 dB", 250.0)] {
            let tick = label(mtm, text, rect(x, 24.0, 60.0, 18.0));
            tick.setFont(Some(&NSFont::systemFontOfSize(11.0)));
            view.addSubview(&tick);
        }
        // SAFETY: The menu retains its view, NativeGain retains it too, and each selector
        // has the expected one-object action signature. Controls do not retain their targets.
        unsafe {
            slider.setTarget(Some(&view));
            slider.setAction(Some(sel!(sliderChanged:)));
        }
        native_item.setView(Some(&view));
        native.setDelegate(Some(ProtocolObject::from_ref(&*view)));
        view.refresh();
        Ok(Self { view, menu: native })
    }

    pub(crate) fn set_gain(&mut self, gain: f32, save_failed: bool) {
        let next = self
            .view
            .ivars()
            .edit
            .borrow_mut()
            .set_gain(gain, save_failed);
        self.view.refresh();
        if let Some(gain) = next {
            (self.view.ivars().notify)(gain);
        }
    }

    pub(crate) fn show(&mut self) -> Result<()> {
        if let Some(window) = self.view.window() {
            window.makeFirstResponder(Some(&*self.view.ivars().slider));
        }
        Ok(())
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.view.ivars().visible.get()
    }
}

impl Drop for NativeGain {
    fn drop(&mut self) {
        self.menu.setDelegate(None);
    }
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(
        objc2_foundation::NSPoint::new(x, y),
        objc2_foundation::NSSize::new(width, height),
    )
}

fn label(mtm: MainThreadMarker, text: &str, frame: NSRect) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFrame(frame);
    label
}
