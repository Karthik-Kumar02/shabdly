//! OS adapters behind one trait, plus the insertion logic they share (spec s3).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldText {
    pub value: String,
    pub is_password: bool,
    pub element_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteOutcome {
    Pasted,
    Copied,
    Blocked,
}

#[derive(Debug, Clone)]
pub enum ClipboardSnapshot {
    Empty,
    Text(String),
    Image(arboard::ImageData<'static>),
}

pub trait Platform: Send + Sync {
    /// The focused text element, or `None` when nothing text-like has focus.
    /// `value` is empty when the app does not expose its text.
    fn focused_field(&self) -> Option<FieldText>;
    fn secure_input(&self) -> bool;
    fn frontmost_app(&self) -> Option<String>;
    fn snapshot_clipboard(&self) -> ClipboardSnapshot;
    fn restore_clipboard(&self, s: ClipboardSnapshot);
    /// Put `text` on the clipboard and send the OS paste shortcut.
    fn paste(&self, text: &str) -> PasteOutcome;
    fn accessibility_trusted(&self, prompt: bool) -> bool;
}

pub mod fake;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

/// The adapter for the OS this build targets.
pub fn current() -> std::sync::Arc<dyn Platform> {
    #[cfg(target_os = "macos")]
    return std::sync::Arc::new(macos::MacPlatform::new());
    #[cfg(target_os = "windows")]
    return std::sync::Arc::new(windows::WindowsPlatform::new());
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return std::sync::Arc::new(fake::FakePlatform::new(vec![None]));
}

/// How long the target app gets to read the clipboard before we restore it.
const PASTE_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

/// Insert `text` into the focused field (spec s3). Password fields and secure
/// input get nothing typed; the text goes to the clipboard so it is not lost.
pub fn insert(platform: &dyn Platform, text: &str) -> PasteOutcome {
    let field = platform.focused_field();
    let copy = |outcome| {
        platform.restore_clipboard(ClipboardSnapshot::Text(text.to_string()));
        outcome
    };
    let Some(field) = field else { return copy(PasteOutcome::Copied) };
    if field.is_password || platform.secure_input() {
        return copy(PasteOutcome::Blocked);
    }
    // ponytail: caret position unknown, so the end of the value stands in for "before the caret".
    let needs_space = field.value.chars().last().is_some_and(|c| !c.is_whitespace());
    let text = if needs_space { format!(" {text}") } else { text.to_string() };
    let before = platform.snapshot_clipboard();
    let outcome = platform.paste(&text);
    if outcome == PasteOutcome::Pasted {
        std::thread::sleep(PASTE_SETTLE);
        platform.restore_clipboard(before);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::fake::FakePlatform;
    use super::*;

    fn field(value: &str, is_password: bool) -> Option<FieldText> {
        Some(FieldText { value: value.into(), is_password, element_id: 7 })
    }

    fn clip_text(p: &FakePlatform) -> Option<String> {
        match p.snapshot_clipboard() {
            ClipboardSnapshot::Text(t) => Some(t),
            _ => None,
        }
    }

    #[test]
    fn insert_blocked_in_password_field() {
        let p = FakePlatform::new(vec![field("", true)]);
        assert_eq!(insert(&p, "secret words"), PasteOutcome::Blocked);
        assert_eq!(clip_text(&p).as_deref(), Some("secret words"));
        assert!(p.pastes().is_empty());

        let p = FakePlatform::new(vec![field("", false)]);
        p.set_secure_input(true);
        assert_eq!(insert(&p, "x"), PasteOutcome::Blocked);
        assert!(p.pastes().is_empty());
    }

    #[test]
    fn insert_copies_when_no_field() {
        let p = FakePlatform::new(vec![None]);
        assert_eq!(insert(&p, "hello"), PasteOutcome::Copied);
        assert_eq!(clip_text(&p).as_deref(), Some("hello"));
        assert!(p.pastes().is_empty());
    }

    #[test]
    fn insert_restores_clipboard() {
        let p = FakePlatform::new(vec![field("", false)]);
        p.restore_clipboard(ClipboardSnapshot::Text("old".into()));
        assert_eq!(insert(&p, "Hello world."), PasteOutcome::Pasted);
        assert_eq!(p.pastes(), vec!["Hello world."]);
        assert_eq!(clip_text(&p).as_deref(), Some("old"));
    }

    #[test]
    fn insert_adds_leading_space() {
        let p = FakePlatform::new(vec![field("Dear team,", false)]);
        insert(&p, "Hello.");
        let p2 = FakePlatform::new(vec![field("Dear team, ", false)]);
        insert(&p2, "Hello.");
        let p3 = FakePlatform::new(vec![field("", false)]);
        insert(&p3, "Hello.");
        assert_eq!(p.pastes(), vec![" Hello."]);
        assert_eq!(p2.pastes(), vec!["Hello."]);
        assert_eq!(p3.pastes(), vec!["Hello."]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn clipboard_snapshot_roundtrip() {
        use std::borrow::Cow;
        let p = super::macos::MacPlatform::new();
        let img = arboard::ImageData { width: 3, height: 2, bytes: Cow::Owned(vec![200u8; 3 * 2 * 4]) };
        arboard::Clipboard::new().unwrap().set_image(img).unwrap();
        let snap = p.snapshot_clipboard();
        p.restore_clipboard(ClipboardSnapshot::Text("temporary".into()));
        p.restore_clipboard(snap);
        let back = arboard::Clipboard::new().unwrap().get_image().unwrap();
        assert_eq!((back.width, back.height), (3, 2));
    }
}
