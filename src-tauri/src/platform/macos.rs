//! macOS adapter: Accessibility for the focused field, Carbon for secure input,
//! arboard for the clipboard, enigo for Cmd+V.
use super::{ClipboardSnapshot, FieldText, PasteOutcome, Platform};
use accessibility_sys::{
    AXIsProcessTrustedWithOptions, AXUIElementCopyAttributeValue, AXUIElementCreateSystemWide, AXUIElementRef,
    AXUIElementSetMessagingTimeout, kAXErrorSuccess, kAXFocusedUIElementAttribute, kAXRoleAttribute,
    kAXSecureTextFieldSubrole, kAXSubroleAttribute, kAXTrustedCheckOptionPrompt, kAXValueAttribute,
};
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

pub struct MacPlatform;

impl MacPlatform {
    pub fn new() -> Self {
        MacPlatform
    }
}

/// Owned AX attribute value, or None when the element does not expose it.
fn attr(el: AXUIElementRef, name: &str) -> Option<CFType> {
    let key = CFString::new(name);
    let mut out: CFTypeRef = std::ptr::null();
    let err = unsafe { AXUIElementCopyAttributeValue(el, key.as_concrete_TypeRef(), &mut out) };
    (err == kAXErrorSuccess && !out.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(out) })
}

fn attr_string(el: AXUIElementRef, name: &str) -> Option<String> {
    attr(el, name)?.downcast::<CFString>().map(|s| s.to_string())
}

fn clipboard() -> Option<arboard::Clipboard> {
    arboard::Clipboard::new().ok()
}

impl Platform for MacPlatform {
    fn focused_field(&self) -> Option<FieldText> {
        let system = unsafe { CFType::wrap_under_create_rule(AXUIElementCreateSystemWide() as CFTypeRef) };
        let sys = system.as_CFTypeRef() as AXUIElementRef;
        // A hung app must not hang dictation.
        unsafe { AXUIElementSetMessagingTimeout(sys, 0.25) };
        let focused = attr(sys, kAXFocusedUIElementAttribute)?;
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
