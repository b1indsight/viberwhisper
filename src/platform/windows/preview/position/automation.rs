//! Window-scoped UI Automation geometry for editors without a Win32 caret.

use anyhow::{Result, ensure};
use windows::Win32::Foundation::{BOOL, HWND as AutomationWindow, POINT as AutomationPoint};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Ole::{
    SafeArrayDestroy, SafeArrayGetDim, SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayGetVartype,
};
use windows::Win32::System::Variant::VT_R8;
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation2, IUIAutomationElement, IUIAutomationTextPattern,
    IUIAutomationTextPattern2, IUIAutomationTextRange, IUIAutomationValuePattern,
    TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, UIA_ComboBoxControlTypeId,
    UIA_EditControlTypeId, UIA_GroupControlTypeId, UIA_IsReadOnlyAttributeId,
    UIA_PaneControlTypeId, UIA_TextControlTypeId, UIA_TextPattern2Id, UIA_TextPatternId,
    UIA_ValuePatternId,
};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetForegroundWindow, GetWindowThreadProcessId,
};

use super::super::thread_focus;
use super::PreviewRect;

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: Constructed only after successful MTA initialization on this worker.
        unsafe { CoUninitialize() };
    }
}

/// Capture only the saved window's focused editable element, then recheck native/UIA focus.
/// All COM objects are released before the worker's apartment; no text/value is retrieved.
/// A click additionally requires affirmative writability and a hit on that element or its
/// non-interactive text descendants, not merely the same browser/renderer HWND.
pub(super) fn capture_anchor(
    focus: HWND,
    click: Option<(i32, i32)>,
) -> Result<Option<PreviewRect>> {
    let window = unsafe { GetAncestor(focus, GA_ROOT) };
    if window.is_null() || unsafe { GetForegroundWindow() } != window {
        return Ok(None);
    }
    let thread = unsafe { GetWindowThreadProcessId(window, std::ptr::null_mut()) };
    if thread_focus(thread)? != focus {
        return Ok(None);
    }
    // SAFETY: Capture runs on the windowless destination worker, never the preview thread.
    // S_OK and S_FALSE both require one CoUninitialize; a changed-apartment error does not.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok()?;
    let _apartment = Apartment;
    let automation: IUIAutomation2 =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }?;
    unsafe {
        automation.SetAutoSetFocus(false)?;
        // These bound individual provider operations, not the complete capture transaction.
        automation.SetConnectionTimeout(500)?;
        automation.SetTransactionTimeout(500)?;
    }
    let root = unsafe { automation.ElementFromHandle(AutomationWindow(window as isize)) }?;
    let element = unsafe { automation.GetFocusedElement() }?;
    // Confirm ancestry before querying any geometry. Native focus alone identifies only
    // the renderer HWND, and searching HasKeyboardFocus can match an ancestor container.
    let walker = unsafe { automation.RawViewWalker() }?;
    let mut ancestor = element.clone();
    let mut belongs_to_window = false;
    for _ in 0..64 {
        if unsafe { automation.CompareElements(&ancestor, &root) }?.as_bool() {
            belongs_to_window = true;
            break;
        }
        ancestor = unsafe { walker.GetParentElement(&ancestor) }?;
    }
    ensure!(
        belongs_to_window,
        "UIA focus is outside the captured window"
    );
    if unsafe { element.CurrentIsPassword()? }.as_bool()
        || unsafe { element.CurrentIsOffscreen()? }.as_bool()
        || !unsafe { element.CurrentIsEnabled()? }.as_bool()
    {
        return Ok(None);
    }
    let control = unsafe { element.CurrentControlType() }?;
    let text =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }.ok();
    let writable = writability(&element, text.as_ref())?;
    let input_control = control == UIA_EditControlTypeId
        || (control == UIA_ComboBoxControlTypeId && writable == Some(true));
    if writable == Some(false) || (!input_control && writable != Some(true)) {
        return Ok(None);
    }
    if let Some(point) = click {
        ensure!(writable == Some(true), "Clicked input is not writable");
        qualify_click(&automation, &element, point)?;
    }
    let caret = match caret_anchor(&element, text.as_ref()) {
        Ok(anchor) => anchor,
        Err(error) => {
            tracing::debug!(%error, "UIA caret unavailable; trying editable control bounds");
            None
        }
    };
    let anchor = if caret.is_some() {
        caret
    } else if input_control {
        // A focused Edit/ComboBox is a reliable input boundary; a Document or browser
        // renderer may span the whole application and is not a useful fallback anchor.
        let bounds = unsafe { element.CurrentBoundingRectangle() }?;
        PreviewRect::new(
            bounds.left as f64,
            bounds.top as f64,
            bounds.right as f64 - bounds.left as f64,
            bounds.bottom as f64 - bounds.top as f64,
        )
    } else {
        None
    };
    // A provider can process focus changes during geometry or hit testing. The clicked
    // input and focused UIA element must still be identical before retaining its anchor.
    if let Some(point) = click {
        qualify_click(&automation, &element, point)?;
        // Recheck the same safety properties as above: an editor can become secure,
        // disabled or read-only without its HWND or UIA identity changing.
        if unsafe { element.CurrentIsPassword()? }.as_bool()
            || unsafe { element.CurrentIsOffscreen()? }.as_bool()
            || !unsafe { element.CurrentIsEnabled()? }.as_bool()
            || writability(&element, text.as_ref())? != Some(true)
        {
            return Ok(None);
        }
    }
    let focused = unsafe { automation.GetFocusedElement() }?;
    Ok((unsafe { GetForegroundWindow() } == window
        && thread_focus(thread)? == focus
        && unsafe { automation.CompareElements(&element, &focused) }?.as_bool()
        && unsafe { element.CurrentHasKeyboardFocus()? }.as_bool())
    .then_some(anchor)
    .flatten())
}

fn writability(
    element: &IUIAutomationElement,
    text: Option<&IUIAutomationTextPattern>,
) -> Result<Option<bool>> {
    if let Ok(value) =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
    {
        Ok(Some(!unsafe { value.CurrentIsReadOnly()? }.as_bool()))
    } else if let Some(text) = text {
        let attribute = unsafe {
            text.DocumentRange()?
                .GetAttributeValue(UIA_IsReadOnlyAttributeId)
        }?;
        Ok(bool::try_from(&attribute).ok().map(|read_only| !read_only))
    } else {
        Ok(None)
    }
}

fn qualify_click(
    automation: &IUIAutomation2,
    focused: &IUIAutomationElement,
    point: (i32, i32),
) -> Result<()> {
    let mut hit = unsafe {
        automation.ElementFromPoint(AutomationPoint {
            x: point.0,
            y: point.1,
        })
    }?;
    let walker = unsafe { automation.RawViewWalker() }?;
    for _ in 0..64 {
        if unsafe { automation.CompareElements(&hit, focused) }?.as_bool() {
            return Ok(());
        }
        let control = unsafe { hit.CurrentControlType() }?;
        // Static text can be a child of a web editor. A button, another input or a
        // scrollbar inside a composite editor must not qualify stale keyboard focus.
        ensure!(
            [
                UIA_TextControlTypeId,
                UIA_GroupControlTypeId,
                UIA_PaneControlTypeId
            ]
            .contains(&control)
                && !unsafe { hit.CurrentIsKeyboardFocusable()? }.as_bool(),
            "Click does not belong to the focused input"
        );
        hit = unsafe { walker.GetParentElement(&hit) }?;
    }
    anyhow::bail!("Click does not belong to the focused input")
}

fn caret_anchor(
    element: &IUIAutomationElement,
    text: Option<&IUIAutomationTextPattern>,
) -> Result<Option<PreviewRect>> {
    if let Ok(pattern) =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id) }
    {
        let mut active = BOOL::default();
        let range = unsafe { pattern.GetCaretRange(&mut active) }?;
        return if active.as_bool() {
            range_bounds(&range)
        } else {
            Ok(None)
        };
    }
    let Some(text) = text else { return Ok(None) };
    let selection = unsafe { text.GetSelection() }?;
    if unsafe { selection.Length() }? != 1 {
        return Ok(None);
    }
    let range = unsafe { selection.GetElement(0) }?;
    unsafe {
        range.MoveEndpointByRange(
            TextPatternRangeEndpoint_Start,
            &range,
            TextPatternRangeEndpoint_End,
        )
    }?;
    range_bounds(&range)
}

/// A provider may return no rectangles for a degenerate range. Do not infer an insertion
/// point from an adjacent character: that is ambiguous for RTL text and wrapped lines.
fn range_bounds(range: &IUIAutomationTextRange) -> Result<Option<PreviewRect>> {
    let bounds = unsafe { range.GetBoundingRectangles() }?;
    if bounds.is_null() {
        return Ok(None);
    }
    let result = (|| {
        // SAFETY: The provider transfers ownership of this SAFEARRAY. Check its element
        // type before copying into f64 storage, and always destroy it after this closure.
        unsafe {
            ensure!(
                SafeArrayGetDim(bounds) == 1 && SafeArrayGetVartype(bounds)? == VT_R8,
                "Invalid UIA rectangle array"
            );
            let first = SafeArrayGetLBound(bounds, 1)?;
            let last = SafeArrayGetUBound(bounds, 1)?;
            if i64::from(last) - i64::from(first) + 1 != 4 {
                return Ok(None);
            }
            let mut rect = [0.0f64; 4];
            for (offset, value) in rect.iter_mut().enumerate() {
                let index = first + offset as i32;
                SafeArrayGetElement(bounds, &index, (value as *mut f64).cast())?;
            }
            // UIA already publishes physical screen pixels, including negative origins.
            Ok(PreviewRect::new(rect[0], rect[1], rect[2], rect[3]))
        }
    })();
    let released = unsafe { SafeArrayDestroy(bounds) };
    let rect = result?;
    released?;
    Ok(rect)
}
