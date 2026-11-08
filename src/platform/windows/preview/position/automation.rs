//! Window-scoped UI Automation geometry for editors without a Win32 caret.

use anyhow::{Result, ensure};
use windows::Win32::Foundation::{BOOL, HWND as AutomationWindow};
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
    UIA_EditControlTypeId, UIA_IsReadOnlyAttributeId, UIA_TextPattern2Id, UIA_TextPatternId,
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
pub(super) fn capture_anchor(focus: HWND) -> Result<Option<PreviewRect>> {
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
    while !unsafe { automation.CompareElements(&ancestor, &root) }?.as_bool() {
        ancestor = unsafe { walker.GetParentElement(&ancestor) }?;
    }
    if unsafe { element.CurrentIsPassword()? }.as_bool()
        || unsafe { element.CurrentIsOffscreen()? }.as_bool()
        || !unsafe { element.CurrentIsEnabled()? }.as_bool()
    {
        return Ok(None);
    }
    let control = unsafe { element.CurrentControlType() }?;
    let text =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }.ok();
    let writable = if let Ok(value) =
        unsafe { element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
    {
        Some(!unsafe { value.CurrentIsReadOnly()? }.as_bool())
    } else if let Some(text) = &text {
        let attribute = unsafe {
            text.DocumentRange()?
                .GetAttributeValue(UIA_IsReadOnlyAttributeId)
        }?;
        bool::try_from(&attribute).ok().map(|read_only| !read_only)
    } else {
        None
    };
    let input_control = control == UIA_EditControlTypeId
        || (control == UIA_ComboBoxControlTypeId && writable == Some(true));
    if writable == Some(false) || (!input_control && writable != Some(true)) {
        return Ok(None);
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
    Ok((unsafe { GetForegroundWindow() } == window
        && thread_focus(thread)? == focus
        && unsafe { element.CurrentHasKeyboardFocus()? }.as_bool())
    .then_some(anchor)
    .flatten())
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
