use std::cell::{Cell, RefCell};
use std::ptr::NonNull;

use anyhow::{Result, anyhow, ensure};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::runtime::ProtocolObject;
use objc2::{DeclaredClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSBezierPath, NSColor, NSEvent, NSEventModifierFlags, NSEventType, NSFont, NSFontAttributeName,
    NSRunningApplication, NSScreen, NSScrollView, NSStringDrawing, NSTextDelegate, NSTextField,
    NSTextInputClient, NSTextView, NSTextViewDelegate, NSView, NSWindow, NSWindowDelegate,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_app_kit::{NSMenu, NSMenuItem};
#[cfg(feature = "integration")]
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_application_services::{AXError, AXUIElement};
use objc2_core_foundation::{CFRetained, CFString, kCFBooleanTrue};
use objc2_foundation::{
    NSDictionary, NSNotification, NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize, NSString,
};

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
    status: Retained<NSTextField>,
    scroll: Retained<NSScrollView>,
    max_width: f64,
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

    unsafe impl NSTextDelegate for PreviewWindow {
        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, _: &NSNotification) {
            self.resize_to_text();
        }
    }

    unsafe impl NSTextViewDelegate for PreviewWindow {}

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
    fn set_message(&self, message: Option<&str>) {
        let status = &self.ivars().status;
        status.setStringValue(&NSString::from_str(message.unwrap_or("")));
        status.setHidden(message.is_none());
        self.resize_to_text();
    }

    /// Fit short candidates, then wrap at the width limit, retaining the top/center anchor.
    fn resize_to_text(&self) {
        let state = self.ivars();
        let font = state.text.font().expect("preview font is configured");
        let attributes: Retained<NSDictionary<NSString, AnyObject>> =
            NSDictionary::from_slices(&[unsafe { NSFontAttributeName }], &[&font]);
        // Measure unwrapped lines with the editor's font so narrow Latin letters,
        // wide glyphs, Chinese and emoji determine the actual compact width.
        let natural_width = state
            .text
            .string()
            .to_string()
            .lines()
            .map(|line| unsafe {
                NSString::from_str(line)
                    .sizeWithAttributes(Some(&attributes))
                    .width
            })
            .fold(0.0_f64, f64::max);
        let width = if state.status.isHidden() {
            (natural_width + 34.0).ceil().clamp(96.0, state.max_width)
        } else {
            state.max_width
        };
        state
            .scroll
            .setFrameSize(NSSize::new(width - 32.0, state.scroll.frame().size.height));
        let used_height = unsafe {
            state
                .text
                .layoutManager()
                .zip(state.text.textContainer())
                .map(|(layout, container)| {
                    layout.ensureLayoutForTextContainer(&container);
                    let used = layout.usedRectForTextContainer(&container);
                    used.origin.y + used.size.height
                })
                .unwrap_or(28.0)
        };
        let editor_height = used_height.clamp(28.0, 196.0);
        let footer = if state.status.isHidden() { 0.0 } else { 44.0 };
        let height = editor_height + 24.0 + footer;
        let mut frame = self.frame();
        frame.origin.x += (frame.size.width - width) / 2.0;
        frame.origin.y += frame.size.height - height;
        frame.size.width = width;
        frame.size.height = height;
        self.setFrame_display(frame, true);
        state.scroll.setFrame(NSRect::new(
            NSPoint::new(16.0, 12.0 + footer),
            NSSize::new(width - 32.0, editor_height),
        ));
        state.text.setMinSize(NSSize::new(0.0, editor_height));
        state.text.setTextContainerInset(NSSize::new(
            0.0,
            ((editor_height - used_height) / 2.0).max(0.0),
        ));
        state.status.setFrame(NSRect::new(
            NSPoint::new(20.0, 10.0),
            NSSize::new(width - 40.0, 38.0),
        ));
    }

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
        text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        text.setTextContainerInset(NSSize::new(0.0, 0.0));
        if let Some(container) = unsafe { text.textContainer() } {
            container.setContainerSize(NSSize::new(width - 32.0, f64::MAX));
            container.setWidthTracksTextView(true);
            container.setLineFragmentPadding(0.0);
        }
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
        let this = PreviewWindow::alloc(mtm).set_ivars(WindowState {
            notify: self.notify.take().expect("preview is created once"),
            session_id: Cell::new(None),
            keys: RefCell::new(ConfirmationKey::default()),
            text: text.clone(),
            status,
            scroll,
            max_width: width,
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
        text.setDelegate(Some(ProtocolObject::from_ref(&*window)));
        window.setContentView(Some(&content));
        window.center();
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
        window.ivars().text.setString(&NSString::from_str(text));
        window.setTitle(&NSString::from_str(&format!(
            "ViberWhisper · {destination}"
        )));
        window.ivars().retry_allowed.set(true);
        window.set_message(warning);
        window
            .ivars()
            .text
            .setSelectedRange(NSRange::new(text.encode_utf16().count(), 0));
        self.focus();
        Ok(())
    }

    pub(crate) fn show_error(&mut self, error: &str, retry_allowed: bool) {
        if let Some(window) = &self.window {
            window.ivars().retry_allowed.set(retry_allowed);
            window.set_message(Some(error));
        }
        self.focus();
    }

    #[allow(deprecated)]
    pub(crate) fn focus(&self) {
        if let Some(window) = &self.window {
            NSApplication::sharedApplication(window.mtm()).activateIgnoringOtherApps(true);
            window.makeKeyAndOrderFront(None);
            window.makeFirstResponder(Some(&window.ivars().text));
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
            window.ivars().text.setString(&NSString::from_str(""));
            window.set_message(None);
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
        let text = &window.ivars().text;
        let replace = |value: &str| unsafe {
            text.insertText_replacementRange(
                &NSString::from_str(value),
                NSRange::new(0, text.string().len_utf16()),
            );
        };
        // Short candidates fit their text; longer candidates reach the width
        // limit before wrapping, and the insertion line stays vertically centered.
        replace("短句");
        let short = window.frame();
        replace(&"中文 English 🎙️ 自动换行。".repeat(20));
        let grown = window.frame();
        ensure!(
            grown.size.height > short.size.height,
            "Bubble did not grow after editing"
        );
        ensure!(
            grown.size.width > short.size.width && grown.size.width <= 420.0,
            "Short candidate did not shrink below the wrapping width"
        );
        ensure!(
            (grown.origin.y + grown.size.height - short.origin.y - short.size.height).abs() < 0.1,
            "Bubble top edge moved while growing"
        );
        replace(&"中文 English 🎙️ 自动换行。".repeat(200));
        ensure!(
            window.frame().size.height <= 220.0,
            "Long text exceeded the bubble height limit"
        );
        replace("短句\n");
        ensure!(
            window.frame().size.height > short.size.height,
            "Trailing newline did not leave room for the caret"
        );
        replace("短句");
        ensure!(
            window.frame().size == short.size,
            "Bubble did not shrink after deletion"
        );
        let caret = unsafe {
            text.firstRectForCharacterRange_actualRange(NSRange::new(0, 0), std::ptr::null_mut())
        };
        ensure!(
            (caret.origin.y + caret.size.height / 2.0 - short.origin.y - short.size.height / 2.0)
                .abs()
                <= 1.0,
            "Short candidate is not vertically centered"
        );
        replace("");
        let empty = window.frame();
        let caret = unsafe {
            text.firstRectForCharacterRange_actualRange(NSRange::new(0, 0), std::ptr::null_mut())
        };
        ensure!(
            (caret.origin.y + caret.size.height / 2.0 - empty.origin.y - empty.size.height / 2.0)
                .abs()
                <= 1.0,
            "Empty candidate caret is not vertically centered"
        );
        replace("iiiiiiiiiiiiiiii");
        let narrow = window.frame().size.width;
        replace("WWWWWWWWWWWWWWWW");
        ensure!(
            window.frame().size.width > narrow,
            "Bubble width ignored native glyph widths"
        );
        replace("短句");
        ensure!(
            text.frame().size.height <= window.ivars().scroll.contentSize().height + 1.0,
            "Deleting long text left a scrollable empty document"
        );
        window.set_message(Some("测试提示"));
        replace(&"自动换行保留提示。".repeat(20));
        ensure!(
            !window.ivars().status.isHidden()
                && window.ivars().status.stringValue().to_string() == "测试提示",
            "Editing lost the delivery message"
        );
        replace("短句");
        ensure!(
            window.frame().size.height == short.size.height + 44.0,
            "Message footer overlapped the resized editor"
        );
        window.set_message(None);
        verify_pending()?;
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
