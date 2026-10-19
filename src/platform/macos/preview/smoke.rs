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
        Ok(())
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
