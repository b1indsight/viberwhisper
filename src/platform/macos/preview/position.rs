//! Captures AX insertion geometry before focus changes and maps display coordinates for AppKit.

use std::ptr::{NonNull, null};

use anyhow::{Result, anyhow, ensure};
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{CFRange, CFRetained, CFString, CFType};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use super::super::accessibility::{copy_attribute, native_error};
use crate::platform::preview::{PreviewRect, preview_origin};

fn attribute(element: &AXUIElement, name: &'static str) -> Result<CFRetained<AXValue>> {
    copy_attribute(
        element,
        &CFString::from_static_str(name),
        "preview position",
    )?
    .downcast::<AXValue>()
    .map_err(|_| anyhow!("输入位置的辅助功能类型无效"))
}

fn caret_bounds(element: &AXUIElement) -> Result<PreviewRect> {
    let selected = attribute(element, "AXSelectedTextRange")?;
    let mut range = CFRange {
        location: 0,
        length: 0,
    };
    // SAFETY: Each out-pointer has the exact layout declared by its AXValueType.
    ensure!(
        unsafe { selected.value(AXValueType::CFRange, NonNull::from(&mut range).cast()) },
        "无法读取选择范围"
    );
    ensure!(range.location >= 0 && range.length >= 0, "输入位置范围无效");
    range.location = range
        .location
        .checked_add(range.length)
        .ok_or_else(|| anyhow!("输入位置范围溢出"))?;
    range.length = 0;
    let parameter = unsafe { AXValue::new(AXValueType::CFRange, NonNull::from(&mut range).cast()) }
        .ok_or_else(|| anyhow!("无法构造输入位置范围"))?;
    let mut result: *const CFType = null();
    let status = unsafe {
        element.copy_parameterized_attribute_value(
            &CFString::from_static_str("AXBoundsForRange"),
            &parameter,
            NonNull::from(&mut result),
        )
    };
    if status != AXError::Success {
        return Err(native_error("preview caret bounds", status).into());
    }
    let pointer = NonNull::new(result.cast_mut()).ok_or_else(|| anyhow!("输入位置为空"))?;
    // SAFETY: A successful AX Copy returns a +1 owned CFType.
    let value = unsafe { CFRetained::from_raw(pointer) }
        .downcast::<AXValue>()
        .map_err(|_| anyhow!("输入位置的辅助功能类型无效"))?;
    let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
    ensure!(
        unsafe { value.value(AXValueType::CGRect, NonNull::from(&mut rect).cast()) },
        "无法读取光标坐标"
    );
    PreviewRect::new(
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    )
    .ok_or_else(|| anyhow!("光标坐标无效"))
}

fn input_bounds(element: &AXUIElement) -> Result<PreviewRect> {
    let role = copy_attribute(
        element,
        &CFString::from_static_str("AXRole"),
        "preview input role",
    )?
    .downcast::<CFString>()
    .map_err(|_| anyhow!("输入框类型无效"))?;
    ensure!(
        matches!(
            role.to_string().as_str(),
            "AXTextField" | "AXTextArea" | "AXComboBox"
        ),
        "控件没有提供输入位置"
    );
    let position = attribute(element, "AXPosition")?;
    let size = attribute(element, "AXSize")?;
    let mut point = NSPoint::new(0.0, 0.0);
    let mut dimensions = NSSize::new(0.0, 0.0);
    // SAFETY: These are the exact CGPoint / CGSize out-pointer types.
    ensure!(
        unsafe { position.value(AXValueType::CGPoint, NonNull::from(&mut point).cast()) }
            && unsafe { size.value(AXValueType::CGSize, NonNull::from(&mut dimensions).cast()) },
        "无法读取输入框坐标"
    );
    PreviewRect::new(point.x, point.y, dimensions.width, dimensions.height)
        .ok_or_else(|| anyhow!("输入框坐标无效"))
}

/// Geometry is advisory: unsupported controls keep the existing centered preview.
pub(super) fn capture_anchor(element: Option<&AXUIElement>) -> Option<PreviewRect> {
    let element = element?;
    match caret_bounds(element) {
        Ok(rect) => return Some(rect),
        Err(error) => tracing::debug!(%error, "Caret bounds unavailable; trying input bounds"),
    }
    match input_bounds(element) {
        Ok(rect) => Some(rect),
        Err(error) => {
            tracing::debug!(%error, "Input bounds unavailable; keeping centered preview");
            None
        }
    }
}

/// AppKit's global Y axis is opposite to AX's; its first screen defines the shared origin.
pub(super) fn screen_geometry(
    anchor: PreviewRect,
    mtm: MainThreadMarker,
) -> Option<(PreviewRect, f64)> {
    let screens = NSScreen::screens(mtm);
    let primary = screens.firstObject()?.frame();
    let top = primary.origin.y + primary.size.height;
    let point = (
        anchor.x + anchor.width / 2.0,
        anchor.y + anchor.height / 2.0,
    );
    screens
        .iter()
        .filter_map(|screen| {
            let frame = screen.visibleFrame();
            let work = PreviewRect::new(
                frame.origin.x,
                top - frame.origin.y - frame.size.height,
                frame.size.width,
                frame.size.height,
            )?;
            let dx = (work.x - point.0)
                .max(0.0)
                .max(point.0 - work.x - work.width);
            let dy = (work.y - point.1)
                .max(0.0)
                .max(point.1 - work.y - work.height);
            Some((work, dx * dx + dy * dy))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(work, _)| (work, top))
}

pub(super) fn origin(
    anchor: PreviewRect,
    size: NSSize,
    work: PreviewRect,
    screen_top: f64,
) -> NSPoint {
    let (x, y) = preview_origin(anchor, (size.width, size.height), work);
    NSPoint::new(x, screen_top - y - size.height)
}
