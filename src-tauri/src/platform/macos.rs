//! macOS adapter: Accessibility for the focused field, Carbon for secure input,
//! arboard for the clipboard, enigo for Cmd+V.
use super::{ClipboardSnapshot, FieldText, PasteOutcome, Platform};
use accessibility_sys::{
    AXError, AXIsProcessTrustedWithOptions, AXUIElementCopyAttributeValue, AXUIElementCreateApplication,
    AXUIElementCreateSystemWide, AXUIElementRef, AXUIElementSetAttributeValue, AXUIElementSetMessagingTimeout,
    kAXErrorAPIDisabled, kAXErrorNoValue, kAXErrorSuccess, kAXFocusedUIElementAttribute, kAXRoleAttribute,
    kAXSecureTextFieldSubrole, kAXSubroleAttribute, kAXTrustedCheckOptionPrompt, kAXValueAttribute,
};
use std::collections::HashSet;
use std::sync::Mutex;
use core_foundation::base::{CFHash, CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
}

/// Roles that can have focus but never take typed text.
const NON_TEXT_ROLES: &[&str] =
    &["AXButton", "AXCheckBox", "AXRadioButton", "AXMenuItem", "AXMenuBarItem", "AXImage", "AXPopUpButton", "AXSlider"];

/// kVK_ANSI_V: the V key position, independent of keyboard layout.
const KEY_V: u32 = 9;

pub struct MacPlatform {
    /// Apps already asked to build their accessibility tree.
    enabled: Mutex<HashSet<i32>>,
}

impl MacPlatform {
    pub fn new() -> Self {
        MacPlatform { enabled: Mutex::new(HashSet::new()) }
    }

    /// The frontmost app's AX element. Electron apps (Slack, VS Code, Claude...)
    /// only expose their text once asked via AXManualAccessibility.
    fn front_app_element(&self) -> Option<CFType> {
        let pid = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication()?.processIdentifier();
        let app = unsafe { CFType::wrap_under_create_rule(AXUIElementCreateApplication(pid) as CFTypeRef) };
        let el = app.as_CFTypeRef() as AXUIElementRef;
        unsafe { AXUIElementSetMessagingTimeout(el, 0.25) };
        if self.enabled.lock().unwrap().insert(pid) {
            let key = CFString::new("AXManualAccessibility");
            unsafe { AXUIElementSetAttributeValue(el, key.as_concrete_TypeRef(), CFBoolean::true_value().as_CFTypeRef()) };
        }
        Some(app)
    }
}

/// Owned AX attribute value, or the AX error when the element does not give it.
fn attr_or_err(el: AXUIElementRef, name: &str) -> Result<CFType, AXError> {
    let key = CFString::new(name);
    let mut out: CFTypeRef = std::ptr::null();
    let err = unsafe { AXUIElementCopyAttributeValue(el, key.as_concrete_TypeRef(), &mut out) };
    if err == kAXErrorSuccess && !out.is_null() { Ok(unsafe { CFType::wrap_under_create_rule(out) }) } else { Err(err) }
}

fn attr(el: AXUIElementRef, name: &str) -> Option<CFType> {
    attr_or_err(el, name).ok()
}

/// The focused element could not be read. "Nothing focused" copies; so does
/// Accessibility being off, because the paste keystroke would be dropped too.
/// Anything else (an app still building its tree) pastes into an unknown field,
/// which observation then skips.
fn field_when_unreadable(err: AXError) -> Option<FieldText> {
    (err != kAXErrorNoValue && err != kAXErrorAPIDisabled)
        .then(|| FieldText { value: String::new(), is_password: false, element_id: 0 })
}

fn attr_string(el: AXUIElementRef, name: &str) -> Option<String> {
    attr(el, name)?.downcast::<CFString>().map(|s| s.to_string())
}

fn clipboard() -> Option<arboard::Clipboard> {
    arboard::Clipboard::new().ok()
}

impl Platform for MacPlatform {
    fn focused_field(&self) -> Option<FieldText> {
        let root = self.front_app_element().unwrap_or_else(|| {
            let sys = unsafe { CFType::wrap_under_create_rule(AXUIElementCreateSystemWide() as CFTypeRef) };
            // A hung app must not hang dictation.
            unsafe { AXUIElementSetMessagingTimeout(sys.as_CFTypeRef() as AXUIElementRef, 0.25) };
            sys
        });
        let focused = match attr_or_err(root.as_CFTypeRef() as AXUIElementRef, kAXFocusedUIElementAttribute) {
            Ok(f) => f,
            Err(e) => return field_when_unreadable(e),
        };
        let el = focused.as_CFTypeRef() as AXUIElementRef;
        let role = attr_string(el, kAXRoleAttribute).unwrap_or_default();
        if NON_TEXT_ROLES.contains(&role.as_str()) {
            return None;
        }
        Some(FieldText {
            value: attr_string(el, kAXValueAttribute).unwrap_or_default(),
            is_password: attr_string(el, kAXSubroleAttribute).as_deref() == Some(kAXSecureTextFieldSubrole),
            element_id: unsafe { CFHash(focused.as_CFTypeRef()) } as u64,
        })
    }

    fn secure_input(&self) -> bool {
        unsafe { IsSecureEventInputEnabled() }
    }

    fn frontmost_app(&self) -> Option<String> {
        let ws = objc2_app_kit::NSWorkspace::sharedWorkspace();
        ws.frontmostApplication()?.localizedName().map(|n| n.to_string())
    }

    fn snapshot_clipboard(&self) -> ClipboardSnapshot {
        let Some(mut c) = clipboard() else { return ClipboardSnapshot::Empty };
        if let Ok(t) = c.get_text() {
            return ClipboardSnapshot::Text(t);
        }
        match c.get_image() {
            Ok(img) => ClipboardSnapshot::Image(img),
            Err(_) => ClipboardSnapshot::Empty,
        }
    }

    fn restore_clipboard(&self, s: ClipboardSnapshot) {
        let Some(mut c) = clipboard() else { return };
        let _ = match s {
            ClipboardSnapshot::Empty => c.clear(),
            ClipboardSnapshot::Text(t) => c.set_text(t),
            ClipboardSnapshot::Image(img) => c.set_image(img),
        };
    }

    fn paste(&self, text: &str) -> PasteOutcome {
        let Some(mut c) = clipboard() else { return PasteOutcome::Copied };
        if c.set_text(text).is_err() {
            return PasteOutcome::Copied;
        }
        let sent = (|| -> Result<(), Box<dyn std::error::Error>> {
            let mut e = Enigo::new(&Settings::default())?;
            e.key(Key::Meta, Direction::Press)?;
            let r = e.raw(KEY_V as u16, Direction::Click);
            e.key(Key::Meta, Direction::Release)?;
            Ok(r?)
        })();
        if sent.is_ok() { PasteOutcome::Pasted } else { PasteOutcome::Copied }
    }

    fn accessibility_trusted(&self, prompt: bool) -> bool {
        let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
        let opts = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::from(prompt))]);
        unsafe { AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef()) }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use accessibility_sys::{kAXErrorAPIDisabled, kAXErrorCannotComplete, kAXErrorNoValue};

    #[test]
    fn unknown_focus_still_pastes_but_no_focus_copies() {
        // Electron apps answer "cannot complete" until their tree is built: paste anyway.
        let f = field_when_unreadable(kAXErrorCannotComplete).expect("paste into the unknown field");
        assert_eq!((f.value.as_str(), f.is_password, f.element_id), ("", false, 0));
        // Nothing focused, or Accessibility off (the paste keystroke would be dropped): copy.
        assert!(field_when_unreadable(kAXErrorNoValue).is_none());
        assert!(field_when_unreadable(kAXErrorAPIDisabled).is_none());
    }
}
