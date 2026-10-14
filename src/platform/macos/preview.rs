use std::cell::{Cell, RefCell};
use std::ptr::NonNull;

use anyhow::{Result, anyhow, ensure};
use objc2::rc::Retained;
#[cfg(feature = "integration")]
use objc2::runtime::AnyObject;
use objc2::runtime::ProtocolObject;
use objc2::{DeclaredClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSBackingStoreType, NSBezierPath, NSColor,
    NSEvent, NSEventModifierFlags, NSEventType, NSFont, NSRunningApplication, NSScreen,
    NSScrollView, NSTextField, NSTextInputClient, NSTextView, NSView, NSWindow, NSWindowDelegate,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_app_kit::{NSMenu, NSMenuItem};
#[cfg(feature = "integration")]
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{CFRetained, CFString, kCFBooleanTrue};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize, NSString};

use super::AccessibilityError;
use super::accessibility::{copy_attribute, native_error, validated_focused_element};
use crate::platform::preview::{
    ConfirmationKey, DeliveryTarget, PreviewAction, PreviewEvent, PreviewKey, PreviewNotify,
    wait_for_activation,
};
use crate::session::SessionId;

/// AX proxies remain on the worker which captured them; the UI receives only the app name.
pub(crate) struct NativeTarget {
    application: Retained<NSRunningApplication>,
    window: CFRetained<AXUIElement>,
    element: Option<CFRetained<AXUIElement>>,
}

fn focused_window(pid: i32) -> Result<CFRetained<AXUIElement>> {
    // SAFETY: A live process ID creates an owned AX proxy; there are no pointer arguments.
    let app = unsafe { AXUIElement::new_application(pid) };
    copy_attribute(
        &app,
        &CFString::from_static_str("AXFocusedWindow"),
        "focused window lookup",
    )?
    .downcast()
    .map_err(|_| anyhow!("无法获取原输入窗口"))
}

impl NativeTarget {
    #[cfg(test)]
    pub(crate) fn permission_denied_error_for_test() -> anyhow::Error {
        native_error("preview target", AXError::APIDisabled).into()
    }

    pub(crate) fn capture() -> Result<Self> {
        objc2::rc::autoreleasepool(|_| {
            let application = NSWorkspace::sharedWorkspace()
                .frontmostApplication()
                .ok_or_else(|| anyhow!("未找到输入目标应用"))?;
            ensure!(
                application.processIdentifier() as u32 != std::process::id(),
                "请先在目标应用中选择输入框"
            );
            let element = match validated_focused_element() {
                Ok(element) => Some(element),
                Err(AccessibilityError::NoFocusedElement) => None,
                Err(error) => return Err(error.into()),
            };
            let window = focused_window(application.processIdentifier())?;
            Ok(Self {
                application,
                window,
                element,
            })
        })
    }
}

impl DeliveryTarget for NativeTarget {
    fn name(&self) -> String {
        self.application
            .localizedName()
            .map_or_else(|| "原应用".into(), |name| name.to_string())
    }

    fn restore(&self) -> Result<()> {
        objc2::rc::autoreleasepool(|_| {
            ensure!(
                !self.application.isTerminated(),
                "原输入应用已关闭，请复制文字后手动输入"
            );
            ensure!(
                self.application
                    .activateWithOptions(NSApplicationActivationOptions::empty()),
                "无法恢复原应用，请复制文字后手动输入"
            );
            // SAFETY: The retained AX window and action string remain live throughout the call.
            let raised = unsafe {
                self.window
                    .perform_action(&CFString::from_static_str("AXRaise"))
            };
            if raised != AXError::Success {
                return Err(
                    anyhow::Error::new(native_error("focused window activation", raised))
                        .context("原输入窗口无法恢复，请复制文字后手动输入"),
                );
            }
            if let Some(element) = &self.element {
                let attribute = CFString::from_static_str("AXFocused");
                let mut settable = 0;
                // SAFETY: `settable` is a live out-pointer. An AX boolean is the documented value type.
                if unsafe {
                    element.is_attribute_settable(&attribute, NonNull::from(&mut settable))
                } == AXError::Success
                    && settable != 0
                {
                    let value =
                        unsafe { kCFBooleanTrue }.ok_or_else(|| anyhow!("无法恢复输入框焦点"))?;
                    let error = unsafe { element.set_attribute_value(&attribute, value) };
                    if error != AXError::Success {
                        return Err(anyhow::Error::new(native_error(
                            "focused element restoration",
                            error,
                        ))
                        .context("无法恢复原输入框，请复制文字后手动输入"));
                    }
                }
            }
            // Activation is asynchronous. Wait on the worker, never on the AppKit thread.
            wait_for_activation(|| self.validate())
        })
    }

    fn validate(&self) -> Result<()> {
        objc2::rc::autoreleasepool(|_| {
            let active = NSWorkspace::sharedWorkspace()
                .frontmostApplication()
                .ok_or_else(|| anyhow!("输入目标失去焦点"))?;
            ensure!(
                active.processIdentifier() == self.application.processIdentifier(),
                "输入目标已切换，请重试或复制文字"
            );
            let window = focused_window(active.processIdentifier())?;
            ensure!(
                *window == *self.window,
                "原输入窗口已变化，请复制文字后手动输入"
            );
            if let Some(expected) = &self.element {
                let focused = validated_focused_element()?;
                ensure!(
                    *focused == **expected,
                    "原输入框已变化，请复制文字后手动输入"
                );
            }
            Ok(())
        })
    }
}

struct WindowState {
    notify: PreviewNotify,
    session_id: Cell<Option<SessionId>>,
    keys: RefCell<ConfirmationKey>,
    text: Retained<NSTextView>,
    retry_allowed: Cell<bool>,
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    struct BubbleView;

    unsafe impl NSObjectProtocol for BubbleView {}

    impl BubbleView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _: NSRect) {
            let mut bounds = self.bounds();
            bounds.origin.x += 0.5;
            bounds.origin.y += 0.5;
            bounds.size.width -= 1.0;
            bounds.size.height -= 1.0;
            let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(bounds, 14.0, 14.0);
            NSColor::textBackgroundColor().setFill();
            path.fill();
            NSColor::separatorColor().setStroke();
            path.setLineWidth(1.0);
            path.stroke();
        }
    }
);

define_class!(
    // SAFETY: NSWindow callbacks run on the AppKit thread; the superclass has no additional invariants.
    #[unsafe(super = NSWindow)]
    #[thread_kind = MainThreadOnly]
    #[ivars = WindowState]
    struct PreviewWindow;

    unsafe impl NSObjectProtocol for PreviewWindow {}

    unsafe impl NSWindowDelegate for PreviewWindow {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _: &NSWindow) -> bool {
            self.notify(PreviewAction::Cancel);
            false
        }
    }

    impl PreviewWindow {
        // Borderless windows need an explicit key-window policy for text editing.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool { true }

        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            let kind = event.r#type();
            let active = self.isKeyWindow() && NSApplication::sharedApplication(self.mtm()).isActive();
            if !active { self.ivars().keys.borrow_mut().reset(); }
            if active && matches!(kind, NSEventType::KeyDown | NSEventType::KeyUp) {
                let action = match event.keyCode() {
                    36 | 76 => Some(PreviewKey::Confirm),
                    53 => Some(PreviewKey::Cancel),
                    _ => None,
                };
                if let Some(action) = action {
                    if kind == NSEventType::KeyUp {
                        let released = self.ivars().keys.borrow_mut().release(action);
                        if let Some(action) = released {
                            self.notify_key(action);
                            return;
                        }
                    }
                    let modifiers = NSEventModifierFlags::Shift | NSEventModifierFlags::Control | NSEventModifierFlags::Option | NSEventModifierFlags::Command;
                    if kind == NSEventType::KeyDown
                        && !event.modifierFlags().intersects(modifiers)
                        && !self.ivars().text.hasMarkedText()
                    {
                        self.ivars().keys.borrow_mut().press(action, event.isARepeat());
                        return;
                    }
                    // Enter/Esc while composing belongs to the user's input method.
                    if kind == NSEventType::KeyDown {
                        self.ivars().keys.borrow_mut().reset();
                    }
                }
            }
            // SAFETY: This is the superclass implementation with the original event.
            unsafe { let _: () = msg_send![super(self), sendEvent: event]; }
        }

        #[unsafe(method(resignKeyWindow))]
        fn resign_key_window(&self) {
            self.ivars().keys.borrow_mut().reset();
            unsafe { let _: () = msg_send![super(self), resignKeyWindow]; }
        }

        #[unsafe(method(becomeKeyWindow))]
        fn become_key_window(&self) {
            self.ivars().keys.borrow_mut().reset();
            unsafe { let _: () = msg_send![super(self), becomeKeyWindow]; }
        }

    }
);

impl PreviewWindow {
    fn notify_key(&self, key: PreviewKey) {
        let action = match key {
            PreviewKey::Confirm if self.ivars().retry_allowed.get() => {
                PreviewAction::Confirm(self.ivars().text.string().to_string())
            }
            PreviewKey::Confirm => return,
            PreviewKey::Cancel => PreviewAction::Cancel,
        };
        self.notify(action);
    }
    fn notify(&self, action: PreviewAction) {
        if let Some(session_id) = self.ivars().session_id.get() {
            (self.ivars().notify)(PreviewEvent { session_id, action });
        }
    }
}

pub(crate) struct NativePreview {
    notify: Option<PreviewNotify>,
    window: Option<Retained<PreviewWindow>>,
    text: Option<Retained<NSTextView>>,
    status: Option<Retained<NSTextField>>,
    scroll: Option<Retained<NSScrollView>>,
}

/// AppKit routes Cmd editing through menu key equivalents to the first responder.
/// Register standard actions once when the lazily created preview first needs them.
fn install_edit_menu(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    let main = app.mainMenu().unwrap_or_else(|| NSMenu::new(mtm));
    let edit = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("编辑"));
    for (title, action, key) in [
        ("剪切", sel!(cut:), "x"),
        ("复制", sel!(copy:), "c"),
        ("粘贴", sel!(paste:), "v"),
        ("全选", sel!(selectAll:), "a"),
    ] {
        // SAFETY: These are standard NSText responder actions; nil targets use the
        // active responder chain, so editing stays in the focused native text view.
        let item = unsafe {
            edit.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
    }
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str("编辑"));
    item.setSubmenu(Some(&edit));
    main.addItem(&item);
    app.setMainMenu(Some(&main));
}

impl NativePreview {
    pub(crate) fn new(notify: PreviewNotify) -> Self {
        Self {
            notify: Some(notify),
            window: None,
            text: None,
            status: None,
            scroll: None,
        }
    }

    fn create(&mut self) -> Result<()> {
        let mtm = MainThreadMarker::new().ok_or_else(|| anyhow!("预览窗口需要在主线程创建"))?;
        install_edit_menu(mtm);
        let available = NSScreen::mainScreen(mtm)
            .map(|s| s.visibleFrame().size)
            .unwrap_or(NSSize::new(800.0, 600.0));
        let width = 420.0_f64.min((available.width - 40.0).max(240.0));
        let height = 84.0;
        let frame = |x, y, w, h| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        let text = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            frame(0.0, 0.0, width - 32.0, height - 24.0),
        );
        text.setEditable(true);
        text.setSelectable(true);
        text.setRichText(false);
        text.setDrawsBackground(false);
        text.setFont(Some(&NSFont::systemFontOfSize(15.0)));
        text.setTextColor(Some(&NSColor::textColor()));
        text.setMinSize(NSSize::new(0.0, height - 24.0));
        text.setMaxSize(NSSize::new(width - 32.0, f64::MAX));
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(false);
        text.setTextContainerInset(NSSize::new(0.0, 0.0));
        if let Some(container) = unsafe { text.textContainer() } {
            container.setContainerSize(NSSize::new(width - 32.0, f64::MAX));
            container.setWidthTracksTextView(true);
        }
        let this = PreviewWindow::alloc(mtm).set_ivars(WindowState {
            notify: self.notify.take().expect("preview is created once"),
            session_id: Cell::new(None),
            keys: RefCell::new(ConfirmationKey::default()),
            text: text.clone(),
            retry_allowed: Cell::new(true),
        });
        // SAFETY: Correct NSWindow initializer. Rust retains the window; closing must not release it.
        let window: Retained<PreviewWindow> = unsafe {
            msg_send![super(this),
                initWithContentRect: frame(0.0, 0.0, width, height),
                styleMask: NSWindowStyleMask::Borderless,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        };
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setTitle(&NSString::from_str("ViberWhisper · 待确认"));
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(true);
        window.setMovableByWindowBackground(true);
        window.setLevel(3);
        window.setHidesOnDeactivate(false);
        window.setDelegate(Some(ProtocolObject::from_ref(&*window)));
        let content: Retained<BubbleView> = unsafe {
            msg_send![super(BubbleView::alloc(mtm).set_ivars(())), initWithFrame: frame(0.0, 0.0, width, height)]
        };
        let status = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        status.setFont(Some(&NSFont::systemFontOfSize(12.0)));
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        status.setHidden(true);
        content.addSubview(&status);
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            frame(16.0, 12.0, width - 32.0, height - 24.0),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setDocumentView(Some(&text));
        content.addSubview(&scroll);
        window.setContentView(Some(&content));
        window.center();
        self.text = Some(text);
        self.status = Some(status);
        self.scroll = Some(scroll);
        self.window = Some(window);
        Ok(())
    }

    pub(crate) fn show(
        &mut self,
        session_id: SessionId,
        text: &str,
        destination: &str,
        warning: Option<&str>,
    ) -> Result<()> {
        if self.window.is_none() {
            self.create()?;
        }
        let window = self.window.as_ref().expect("created window");
        window.ivars().session_id.set(Some(session_id));
        window.ivars().keys.borrow_mut().reset();
        self.text
            .as_ref()
            .expect("created text view")
            .setString(&NSString::from_str(text));
        window.setTitle(&NSString::from_str(&format!(
            "ViberWhisper · {destination}"
        )));
        window.ivars().retry_allowed.set(true);
        self.layout(warning);
        self.text
            .as_ref()
            .expect("created text view")
            .setSelectedRange(NSRange::new(text.encode_utf16().count(), 0));
        self.focus();
        Ok(())
    }

    pub(crate) fn show_error(&mut self, error: &str, retry_allowed: bool) {
        if let Some(window) = &self.window {
            window.ivars().retry_allowed.set(retry_allowed);
        }
        self.layout(Some(error));
        self.focus();
    }

    fn layout(&self, message: Option<&str>) {
        let Some(window) = &self.window else {
            return;
        };
        let text = self.text.as_ref().expect("created text view");
        let width = window.frame().size.width;
        let used_height = unsafe {
            text.layoutManager()
                .zip(text.textContainer())
                .map(|(layout, container)| {
                    layout.glyphRangeForTextContainer(&container);
                    layout.usedRectForTextContainer(&container).size.height
                })
                .unwrap_or(60.0)
        };
        let footer = if message.is_some() { 44.0 } else { 0.0 };
        let height = used_height.clamp(28.0, 196.0) + 24.0 + footer;
        let mut frame = window.frame();
        frame.origin.y += frame.size.height - height;
        frame.size.height = height;
        window.setFrame_display(frame, true);
        self.scroll
            .as_ref()
            .expect("created scroll view")
            .setFrame(NSRect::new(
                NSPoint::new(16.0, 12.0 + footer),
                NSSize::new(width - 32.0, height - 24.0 - footer),
            ));
        text.setMinSize(NSSize::new(0.0, height - 24.0 - footer));
        let status = self.status.as_ref().expect("created status");
        status.setStringValue(&NSString::from_str(message.unwrap_or("")));
        status.setFrame(NSRect::new(
            NSPoint::new(20.0, 10.0),
            NSSize::new(width - 40.0, 38.0),
        ));
        status.setHidden(message.is_none());
    }

    #[allow(deprecated)]
    pub(crate) fn focus(&self) {
        if let Some(window) = &self.window {
            NSApplication::sharedApplication(window.mtm()).activateIgnoringOtherApps(true);
            window.makeKeyAndOrderFront(None);
            if let Some(text) = &self.text {
                window.makeFirstResponder(Some(text));
            }
        }
    }
    pub(crate) fn hide(&self) {
        if let Some(window) = &self.window {
            window.orderOut(None);
        }
    }
    pub(crate) fn close(&mut self) {
        if let Some(window) = &self.window {
            window.ivars().session_id.set(None);
            window.ivars().keys.borrow_mut().reset();
            window.orderOut(None);
        }
        if let Some(text) = &self.text {
            text.setString(&NSString::from_str(""));
        }
        if let Some(status) = &self.status {
            status.setStringValue(&NSString::from_str(""));
        }
    }

    #[cfg(feature = "integration")]
    pub(crate) fn smoke_confirmation(
        &self,
        edited_text: &str,
        verify_pending: &dyn Fn() -> Result<()>,
    ) -> Result<bool> {
        use objc2::ClassType;
        let window = self
            .window
            .as_ref()
            .ok_or_else(|| anyhow!("Preview was not created"))?;
        if !window.isKeyWindow() || !NSApplication::sharedApplication(window.mtm()).isActive() {
            return Ok(false);
        }
        let text = self.text.as_ref().expect("created text view");
        let app = NSApplication::sharedApplication(window.mtm());
        let send = |kind, modifiers, repeat, key_code, characters: &str| -> Result<()> {
            let characters = NSString::from_str(characters);
            // SAFETY: Correct NSEvent factory signature; context is documented to be nil.
            let event: Option<Retained<NSEvent>> = unsafe {
                msg_send![NSEvent::class(),
                    keyEventWithType: kind,
                    location: NSPoint::new(0.0, 0.0),
                    modifierFlags: modifiers,
                    timestamp: 0.0_f64,
                    windowNumber: window.windowNumber(),
                    context: Option::<&AnyObject>::None,
                    characters: &*characters,
                    charactersIgnoringModifiers: &*characters,
                    isARepeat: repeat,
                    keyCode: key_code
                ]
            };
            let event = event.ok_or_else(|| anyhow!("Could not create native key event"))?;
            app.sendEvent(&event);
            Ok(())
        };
        // Enter held in another window must not be adopted after focus acquisition.
        for _ in 0..5 {
            send(
                NSEventType::KeyDown,
                NSEventModifierFlags::empty(),
                true,
                36_u16,
                "\r",
            )?;
        }
        send(
            NSEventType::KeyUp,
            NSEventModifierFlags::empty(),
            false,
            36_u16,
            "\r",
        )?;
        verify_pending()?;
        // Exercise actual marked text: selecting a Chinese candidate must not submit
        // the bubble. Modified Return must remain available for multiline edits.
        unsafe {
            text.setMarkedText_selectedRange_replacementRange(
                &NSString::from_str("候选"),
                NSRange::new(2, 0),
                NSRange::new(0, text.string().len_utf16()),
            );
        }
        ensure!(
            text.hasMarkedText(),
            "Native editor did not enter composition"
        );
        send(
            NSEventType::KeyDown,
            NSEventModifierFlags::empty(),
            false,
            36_u16,
            "\r",
        )?;
        send(
            NSEventType::KeyUp,
            NSEventModifierFlags::empty(),
            false,
            36_u16,
            "\r",
        )?;
        verify_pending()?;
        text.unmarkText();
        send(
            NSEventType::KeyDown,
            NSEventModifierFlags::Shift,
            false,
            36_u16,
            "\r",
        )?;
        send(
            NSEventType::KeyUp,
            NSEventModifierFlags::Shift,
            false,
            36_u16,
            "\r",
        )?;
        verify_pending()?;
        // Replace through the editor's input-client API, as a user correction would.
        unsafe {
            text.insertText_replacementRange(
                &NSString::from_str(edited_text),
                NSRange::new(0, text.string().len_utf16()),
            );
        }
        // Use application event dispatch: directly invoking text actions would
        // miss the absent Edit-menu bindings that caused the shortcut regression.
        send(
            NSEventType::KeyDown,
            NSEventModifierFlags::Command,
            false,
            0_u16,
            "a",
        )?;
        ensure!(
            text.selectedRange().length == text.string().len_utf16(),
            "Cmd+A did not select the text"
        );
        send(
            NSEventType::KeyDown,
            NSEventModifierFlags::Command,
            false,
            8_u16,
            "c",
        )?;
        let pasteboard = NSPasteboard::generalPasteboard();
        let copied = pasteboard.stringForType(unsafe { NSPasteboardTypeString });
        ensure!(
            copied.is_some_and(|value| value.to_string() == edited_text),
            "Cmd+C did not copy the text"
        );
        send(
            NSEventType::KeyDown,
            NSEventModifierFlags::Command,
            false,
            7_u16,
            "x",
        )?;
        ensure!(text.string().is_empty(), "Cmd+X did not cut the selection");
        send(
            NSEventType::KeyDown,
            NSEventModifierFlags::Command,
            false,
            9_u16,
            "v",
        )?;
        ensure!(
            text.string().to_string() == edited_text,
            "Cmd+V did not restore the text"
        );
        verify_pending()?;
        for index in 0..5 {
            send(
                NSEventType::KeyDown,
                NSEventModifierFlags::empty(),
                index > 0,
                36_u16,
                "\r",
            )?;
        }
        verify_pending()?;
        send(
            NSEventType::KeyUp,
            NSEventModifierFlags::empty(),
            false,
            36_u16,
            "\r",
        )?;
        Ok(true)
    }
}
