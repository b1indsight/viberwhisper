use std::ptr::NonNull;

use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement};
use objc2_core_foundation::{CFRetained, CFString, CFType};

use super::{AccessibilityError, AccessibilityInsert, AccessibilityWriter};

const FOCUSED_UI_ELEMENT_ATTRIBUTE: &str = "AXFocusedUIElement";
const SELECTED_TEXT_ATTRIBUTE: &str = "AXSelectedText";
const SUBROLE_ATTRIBUTE: &str = "AXSubrole";
const SECURE_TEXT_FIELD_SUBROLE: &str = "AXSecureTextField";

pub(super) struct NativeAccessibility;

fn native_error(operation: &'static str, error: AXError) -> AccessibilityError {
    if error == AXError::APIDisabled {
        AccessibilityError::PermissionDenied
    } else {
        AccessibilityError::Native {
            operation,
            code: error.0,
        }
    }
}

fn is_unsupported(error: AXError) -> bool {
    matches!(
        error,
        AXError::AttributeUnsupported | AXError::NoValue | AXError::NotImplemented
    )
}

/// Copies one AX attribute while preserving the Core Foundation create-rule ownership returned by
/// `AXUIElementCopyAttributeValue`.
fn copy_attribute(
    element: &AXUIElement,
    attribute: &CFString,
    operation: &'static str,
) -> Result<CFRetained<CFType>, AccessibilityError> {
    let mut value: *const CFType = std::ptr::null();
    // SAFETY: `value` is a valid out-pointer for the duration of the call. A successful Copy call
    // returns a retained Core Foundation object, which is transferred into `CFRetained` below.
    let error = unsafe { element.copy_attribute_value(attribute, NonNull::from(&mut value)) };
    if error != AXError::Success {
        return Err(native_error(operation, error));
    }

    let value = NonNull::new(value.cast_mut()).ok_or(AccessibilityError::Native {
        operation,
        code: AXError::Failure.0,
    })?;
    // SAFETY: The successful AX Copy call returned a non-null object with +1 retain ownership.
    Ok(unsafe { CFRetained::from_raw(value) })
}

fn focused_element() -> Result<CFRetained<AXUIElement>, AccessibilityError> {
    // SAFETY: This function has no pointer parameters or caller-side preconditions.
    if !unsafe { AXIsProcessTrusted() } {
        return Err(AccessibilityError::PermissionDenied);
    }

    // SAFETY: Creating the system-wide AX object has no caller-side preconditions.
    let system = unsafe { AXUIElement::new_system_wide() };
    let focused_attribute = CFString::from_static_str(FOCUSED_UI_ELEMENT_ATTRIBUTE);
    match copy_attribute(&system, &focused_attribute, "focused element lookup") {
        Ok(value) => {
            value
                .downcast::<AXUIElement>()
                .map_err(|_| AccessibilityError::UnexpectedType {
                    operation: "focused element lookup",
                })
        }
        Err(AccessibilityError::Native { code, .. })
            if code == AXError::NoValue.0
                || code == AXError::AttributeUnsupported.0
                || code == AXError::NotImplemented.0 =>
        {
            Err(AccessibilityError::NoFocusedElement)
        }
        Err(error) => Err(error),
    }
}

fn reject_secure_control(element: &AXUIElement) -> Result<(), AccessibilityError> {
    let subrole_attribute = CFString::from_static_str(SUBROLE_ATTRIBUTE);
    let subrole = match copy_attribute(element, &subrole_attribute, "subrole lookup") {
        Ok(value) => {
            value
                .downcast::<CFString>()
                .map_err(|_| AccessibilityError::UnexpectedType {
                    operation: "subrole lookup",
                })?
        }
        Err(AccessibilityError::Native { code, .. })
            if code == AXError::NoValue.0
                || code == AXError::AttributeUnsupported.0
                || code == AXError::NotImplemented.0 =>
        {
            return Ok(());
        }
        Err(error) => return Err(error),
    };

    if subrole.to_string() == SECURE_TEXT_FIELD_SUBROLE {
        Err(AccessibilityError::SecureControl)
    } else {
        Ok(())
    }
}

fn validated_focused_element() -> Result<CFRetained<AXUIElement>, AccessibilityError> {
    let focused = focused_element()?;
    reject_secure_control(&focused)?;
    Ok(focused)
}

impl AccessibilityWriter for NativeAccessibility {
    fn validate_paste_destination(&self) -> Result<(), AccessibilityError> {
        validated_focused_element().map(drop)
    }

    fn insert_selected_text(&self, text: &str) -> Result<AccessibilityInsert, AccessibilityError> {
        let focused = validated_focused_element()?;

        let selected_text_attribute = CFString::from_static_str(SELECTED_TEXT_ATTRIBUTE);
        let mut settable = 0;
        // SAFETY: `settable` is a valid out-pointer for the duration of this call.
        let error = unsafe {
            focused.is_attribute_settable(&selected_text_attribute, NonNull::from(&mut settable))
        };
        if is_unsupported(error) {
            return Ok(AccessibilityInsert::Unsupported(
                "selected text is unsupported",
            ));
        }
        if error != AXError::Success {
            return Err(native_error("selected-text settable check", error));
        }
        if settable == 0 {
            return Ok(AccessibilityInsert::Unsupported(
                "selected text is not settable",
            ));
        }

        let text = CFString::from_str(text);
        // SAFETY: `CFString` is a supported AX attribute value type and both references remain
        // valid for the duration of the synchronous call.
        let error = unsafe { focused.set_attribute_value(&selected_text_attribute, &text) };
        if is_unsupported(error) {
            return Ok(AccessibilityInsert::Unsupported(
                "selected-text assignment is unsupported",
            ));
        }
        if error != AXError::Success {
            return Err(native_error("selected-text assignment", error));
        }

        Ok(AccessibilityInsert::Inserted)
    }
}
