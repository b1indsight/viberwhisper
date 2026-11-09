//! Native editing and delivery checks for the integration demo.

use super::*;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};

impl NativePreview {
    /// Snapshot this fixture's own rendered content, never another application.
    pub(crate) fn smoke_capture(&self, path: &std::path::Path) -> Result<()> {
        let window = self
            .window
            .as_ref()
            .ok_or_else(|| anyhow!("Preview not created"))?;
        let view = window
            .contentView()
            .ok_or_else(|| anyhow!("Preview has no content"))?;
        window.displayIfNeeded();
        let bitmap = view
            .bitmapImageRepForCachingDisplayInRect(view.bounds())
            .ok_or_else(|| anyhow!("Could not capture preview"))?;
        view.cacheDisplayInRect_toBitmapImageRep(view.bounds(), &bitmap);
        // SAFETY: Empty properties use AppKit's documented default PNG encoding;
        // the immutable data is retained until its bytes have been written.
        let png = unsafe {
            bitmap.representationUsingType_properties(
                objc2_app_kit::NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }
        .ok_or_else(|| anyhow!("Could not encode preview PNG"))?;
        std::fs::write(path, unsafe { png.as_bytes_unchecked() })?;
        Ok(())
    }

    /// Exercise the native key dispatcher without posting keys to another app.
    /// The caller checks that Enter emitted nothing before Esc emits cancellation.
    pub(crate) fn smoke_error(&mut self, verify_pending: &dyn Fn() -> Result<()>) -> Result<()> {
        use objc2::ClassType;

        let window = self
            .window
            .as_ref()
            .ok_or_else(|| anyhow!("Error preview was not created"))?
            .clone();
        let state = window.ivars();
        ensure!(
            state.text.string().is_empty()
                && !state.text.isEditable()
                && state.text.isSelectable()
                && !state.retry_allowed.get(),
            "Error preview retained text, accepted edits, or enabled confirmation"
        );
        ensure!(
            !state.status.isHidden() && state.status.stringValue().to_string().contains("Esc"),
            "Error preview did not explain cancellation"
        );
        let compact_height = state.status.frame().size.height + 6.0;
        ensure!(
            state.scroll.isHidden() && (window.frame().size.height - compact_height).abs() < 1.0,
            "Empty error preview still reserves a blank editor row"
        );
        let warning = state.status.stringValue().to_string();
        let compact_width = window.frame().size.width;
        ensure!(
            compact_width < state.max_width,
            "Short error warning still forces the maximum bubble width"
        );
        // Equal character counts must fit native glyph widths, including a longer second line.
        let mut widths = [0.0; 2];
        for (index, message) in [
            "错误。\niiiiiiiiiiiiiiiiiiiiiiii · Esc",
            "错误。\nWWWWWWWWWWWWWWWWWWWWWWWW · Esc",
        ]
        .into_iter()
        .enumerate()
        {
            self.update_text("", false, Some(message))?;
            widths[index] = window.frame().size.width;
            ensure!(
                (window.frame().size.height - compact_height).abs() < 1.0,
                "A fitting two-line warning wrapped or changed the compact height"
            );
        }
        ensure!(
            widths[0] < widths[1] && widths[1] < state.max_width,
            "Error width ignored native glyphs or the longest second line"
        );
        let long_warning = format!(
            "{}\n按 Esc 关闭。",
            "识别服务暂时不可用，请检查网络连接后重试。".repeat(6)
        );
        self.update_text("", false, Some(&long_warning))?;
        ensure!(
            (window.frame().size.width - state.max_width).abs() < 1.0
                && window.frame().size.height > compact_height
                && state.scroll.isHidden(),
            "A long error warning did not wrap at the bubble width limit"
        );
        println!(
            "Native error warning widths: {compact_width}; proportional glyphs: {widths:?}; wrapped: {:?}",
            window.frame().size
        );
        self.update_text("", false, Some(&warning))?;
        // A partial result and a user's emptied editable candidate must stay editable;
        // whitespace-only failures still have no usable body. Reuse the same window.
        for (text, ready, collapsed) in [
            ("部分识别结果", true, false),
            ("", true, false),
            (" \n ", false, true),
            ("", false, true),
        ] {
            self.update_text(text, ready, Some(&warning))?;
            ensure!(
                state.scroll.isHidden() == collapsed
                    && state.text.isEditable() == ready
                    && if collapsed {
                        (window.frame().size.height - compact_height).abs() < 1.0
                            && (window.frame().size.width - compact_width).abs() < 1.0
                    } else {
                        window.frame().size.height >= compact_height + 28.0
                            && (window.frame().size.width - state.max_width).abs() < 1.0
                    },
                "Error layout lost its body visibility or editable recovery; ready={ready}"
            );
            if ready && text.is_empty() {
                self.show_error(&warning, false);
                ensure!(
                    !state.scroll.isHidden(),
                    "A blocked delivery retry hid an editable candidate"
                );
            }
        }
        self.focus();
        let text_responder: &objc2_app_kit::NSResponder = &state.text;
        ensure!(
            window.isKeyWindow()
                && NSApplication::sharedApplication(window.mtm()).isActive()
                && window.firstResponder().as_deref() != Some(text_responder),
            "Error preview did not focus its window instead of the hidden editor"
        );
        for (key_code, characters) in [(36_u16, "\r"), (53_u16, "\u{1b}")] {
            for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
                let characters = NSString::from_str(characters);
                // SAFETY: Factory arguments match NSEvent; the event stays in our own window.
                let event: Option<Retained<NSEvent>> = unsafe {
                    msg_send![NSEvent::class(),
                        keyEventWithType: kind,
                        location: NSPoint::new(0.0, 0.0),
                        modifierFlags: NSEventModifierFlags::empty(),
                        timestamp: 0.0_f64,
                        windowNumber: window.windowNumber(),
                        context: Option::<&AnyObject>::None,
                        characters: &*characters,
                        charactersIgnoringModifiers: &*characters,
                        isARepeat: false,
                        keyCode: key_code
                    ]
                };
                let event =
                    event.ok_or_else(|| anyhow!("Could not create native error key event"))?;
                window.sendEvent(&event);
            }
            if key_code == 36 {
                verify_pending()?;
            }
        }
        Ok(())
    }

    pub(crate) fn smoke_streaming(&mut self) -> Result<()> {
        {
            let window = self
                .window
                .as_ref()
                .ok_or_else(|| anyhow!("Streaming preview was not created"))?;
            let text = &window.ivars().text;
            ensure!(
                !text.isEditable() && text.isSelectable() && !window.ivars().retry_allowed.get(),
                "Streaming preview accepted edits or confirmation"
            );
            window.notify_key(PreviewKey::Confirm);
            window
                .ivars()
                .keys
                .borrow_mut()
                .press(PreviewKey::Confirm, false);
        }

        // The replacement starts at shared word/grapheme boundaries, so a prefix
        // selection remains selected while English, Chinese, emoji, and newlines
        // receive final model corrections.
        self.window
            .as_ref()
            .expect("streaming preview was checked")
            .ivars()
            .text
            .setString(&NSString::from_str("I like coding"));
        self.window
            .as_ref()
            .expect("streaming preview was checked")
            .ivars()
            .text
            .setSelectedRange(NSRange::new(0, 2));
        self.update_text("I love coding", false, None)?;
        let text = &self
            .window
            .as_ref()
            .expect("streaming preview was checked")
            .ivars()
            .text;
        ensure!(
            text.string().to_string() == "I love coding"
                && text.selectedRange() == NSRange::new(0, 2),
            "Streaming correction rewrote the selected prefix"
        );
        self.window
            .as_ref()
            .expect("streaming preview was checked")
            .ivars()
            .text
            .setString(&NSString::from_str("我们明天下午去北京。"));
        self.update_text("我们明天上午去上海。", false, None)?;
        let text = &self
            .window
            .as_ref()
            .expect("streaming preview was checked")
            .ivars()
            .text;
        ensure!(
            text.string().to_string() == "我们明天上午去上海。",
            "Streaming correction did not apply Chinese tail replacement"
        );
        self.update_text("", false, None)?;
        let text = &self
            .window
            .as_ref()
            .expect("streaming preview was checked")
            .ivars()
            .text;
        ensure!(
            text.string().is_empty() && !text.isEditable() && !text.isHiddenOrHasHiddenAncestor(),
            "An empty aggregate did not retract the read-only provisional text"
        );
        self.window
            .as_ref()
            .expect("streaming preview was checked")
            .ivars()
            .text
            .setString(&NSString::from_str("👩‍💻\nold"));
        self.update_text("👩‍💻\nnew", true, None)?;
        let window = self.window.as_ref().expect("streaming preview was checked");
        let text = &window.ivars().text;
        ensure!(
            text.string().to_string() == "👩‍💻\nnew"
                && text.isEditable()
                && window.ivars().retry_allowed.get(),
            "Streaming preview did not become an editable final candidate"
        );
        ensure!(
            window
                .ivars()
                .keys
                .borrow_mut()
                .release(PreviewKey::Confirm)
                .is_none(),
            "A generating Enter survived the ready transition"
        );
        self.smoke_relocation()?;
        Ok(())
    }

    /// Late placement must survive both a manually moved bubble and existing user edits.
    /// Keep native selection/scroll/undo evidence, not just the string after relocation.
    fn smoke_relocation(&mut self) -> Result<()> {
        let window = self
            .window
            .as_ref()
            .ok_or_else(|| anyhow!("No preview"))?
            .clone();
        let state = window.ivars();
        let saved_anchor = state.anchor.get();
        let saved_title = window.title();
        let saved_text = state.text.string().to_string();
        let saved_warning =
            (!state.status.isHidden()).then(|| state.status.stringValue().to_string());
        let saved_ready = state.retry_allowed.get();
        let saved_undo = state.text.allowsUndo();
        state.text.setAllowsUndo(true);
        let reference =
            saved_anchor.unwrap_or_else(|| PreviewRect::new(0.0, 0.0, 1.0, 18.0).unwrap());
        let (work, top) = position::screen_geometry(reference, window.mtm())
            .ok_or_else(|| anyhow!("No preview work area"))?;
        let long = "hello 😀 中文\n".repeat(40);
        let result = (|| {
            for ready in [false, true] {
                self.update_text(&long, ready, Some("识别提醒\n无法获取输入目标"))?;
                let undo = state
                    .text
                    .undoManager()
                    .ok_or_else(|| anyhow!("No native undo manager"))?;
                if ready {
                    undo.beginUndoGrouping();
                    unsafe {
                        state.text.insertText_replacementRange(
                            &NSString::from_str("user correction"),
                            NSRange::new(state.text.string().len_utf16(), 0),
                        );
                    }
                    undo.endUndoGrouping();
                }
                state.text.setSelectedRange(NSRange::new(6, 2)); // A whole emoji.
                state.text.scrollPoint(NSPoint::new(0.0, 60.0));
                let selection = state.text.selectedRange();
                let visible = state.scroll.documentVisibleRect().origin;
                ensure!(
                    visible.y > 0.0,
                    "Relocation fixture did not scroll its long preview"
                );
                let text = state.text.string().to_string();
                let key = window.isKeyWindow();
                let responder = window.firstResponder();
                let active = NSWorkspace::sharedWorkspace()
                    .frontmostApplication()
                    .map(|app| app.processIdentifier());
                let frame = window.frame();
                window.setFrameOrigin(NSPoint::new(frame.origin.x + 20.0, frame.origin.y + 20.0));
                let anchor = PreviewRect::new(work.x + 51.0, work.y + 93.0, 1.0, 18.0).unwrap();
                let warning = ready.then_some("识别提醒");
                self.relocate("Late input target", anchor, warning)?;
                let expected = position::origin(anchor, window.frame().size, work, top);
                let actual = window.frame();
                let scroll = state.scroll.documentVisibleRect().origin;
                ensure!(
                    state.text.string().to_string() == text
                        && state.text.selectedRange() == selection
                        && (scroll.x - visible.x).abs() <= 1.0
                        && (scroll.y - visible.y).abs() <= 1.0
                        && state.text.isEditable() == ready
                        && state.retry_allowed.get() == ready
                        && undo.canUndo() == ready
                        && window.isKeyWindow() == key
                        && window.firstResponder() == responder
                        && NSWorkspace::sharedWorkspace()
                            .frontmostApplication()
                            .map(|app| app.processIdentifier())
                            == active,
                    "Relocation reset native text, selection, scroll, undo, editing state or focus"
                );
                ensure!(
                    (actual.origin.x - expected.x).abs() <= 1.0
                        && (actual.origin.y - expected.y).abs() <= 1.0
                        && window.title().to_string() == "ViberWhisper · Late input target"
                        && state.status.stringValue().to_string() == warning.unwrap_or("")
                        && state.status.isHidden() == warning.is_none(),
                    "Relocation lost its new anchor, destination or source warning"
                );
                let editor = state.scroll.frame();
                ensure!(
                    editor.origin.x == 10.0
                        && actual.size.width - editor.size.width == 20.0
                        && actual.size.height - editor.origin.y - editor.size.height == 3.0
                        && (warning.is_some() || editor.origin.y == 3.0),
                    "Relocated preview lost its compact body margins"
                );
                if ready {
                    undo.undo();
                    ensure!(
                        state.text.string().to_string() == long,
                        "Relocation broke native undo"
                    );
                    let status = state.status.frame();
                    ensure!(
                        status.origin.x == 14.0
                            && actual.size.width - status.size.width == 28.0
                            && status.origin.y == 3.0
                            && editor.origin.y - status.origin.y - status.size.height == 3.0,
                        "Relocated preview lost its compact warning margins"
                    );
                }
            }
            Ok(())
        })();
        state.anchor.set(saved_anchor);
        state.last_origin.set(None);
        window.setTitle(&saved_title);
        let restored = self.update_text(&saved_text, saved_ready, saved_warning.as_deref());
        state.text.setAllowsUndo(saved_undo);
        result?;
        restored
    }

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
        let anchor = window
            .ivars()
            .anchor
            .get()
            .ok_or_else(|| anyhow!("Smoke target did not expose caret bounds"))?;
        ensure!(
            anchor.width <= 16.0 && anchor.height <= 128.0,
            "Smoke target exposed input bounds, not precise caret bounds"
        );
        let (work, top) = position::screen_geometry(anchor, window.mtm())
            .ok_or_else(|| anyhow!("Could not locate the caret's display"))?;
        let actual = window.frame();
        let (expected_x, expected_y) = crate::platform::preview::preview_origin(
            anchor,
            (actual.size.width, actual.size.height),
            work,
        );
        ensure!(
            (actual.origin.x - expected_x).abs() <= 1.0
                && (top - actual.origin.y - actual.size.height - expected_y).abs() <= 1.0,
            "Native preview was not placed by the captured caret"
        );
        println!("Captured caret bounds: {anchor:?}");
        let text = &window.ivars().text;
        let replace = |value: &str| unsafe {
            text.insertText_replacementRange(
                &NSString::from_str(value),
                NSRange::new(0, text.string().len_utf16()),
            );
        };
        // Native moves outside layout represent user/system positioning. Editing
        // after a drag must preserve that location instead of jumping to the caret.
        let placed = window.frame();
        window.setFrameOrigin(NSPoint::new(placed.origin.x + 20.0, placed.origin.y + 20.0));
        let moved = window.frame();
        replace("短句");
        let edited = window.frame();
        ensure!(
            window.ivars().anchor.get().is_none()
                && (edited.origin.x + edited.size.width / 2.0
                    - moved.origin.x
                    - moved.size.width / 2.0)
                    .abs()
                    <= 1.0
                && (edited.origin.y + edited.size.height - moved.origin.y - moved.size.height)
                    .abs()
                    <= 1.0,
            "Editing after a manual move jumped back to the caret"
        );
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
            window.frame().size.height <= 202.0,
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
            window.frame().size.height == short.size.height + 35.0,
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
