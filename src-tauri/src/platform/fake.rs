//! Scripted platform for tests: focused-field values are served in order (the
//! last one repeats), pastes are recorded, and the clipboard is in memory.
use super::{ClipboardSnapshot, FieldText, PasteOutcome, Platform};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct FakePlatform {
    fields: Mutex<VecDeque<Option<FieldText>>>,
    pastes: Mutex<Vec<String>>,
    clipboard: Mutex<ClipboardSnapshot>,
    secure: AtomicBool,
}

impl FakePlatform {
    pub fn new(fields: Vec<Option<FieldText>>) -> Self {
        FakePlatform {
            fields: Mutex::new(fields.into()),
            pastes: Mutex::new(vec![]),
            clipboard: Mutex::new(ClipboardSnapshot::Empty),
            secure: AtomicBool::new(false),
        }
    }

    /// Queue more focused-field values (what the user does after a paste).
    pub fn push_fields(&self, more: Vec<Option<FieldText>>) {
        self.fields.lock().unwrap().extend(more);
    }

    pub fn set_secure_input(&self, on: bool) {
        self.secure.store(on, Ordering::SeqCst);
    }

    pub fn pastes(&self) -> Vec<String> {
        self.pastes.lock().unwrap().clone()
    }
}

impl Platform for FakePlatform {
    fn focused_field(&self) -> Option<FieldText> {
        let mut q = self.fields.lock().unwrap();
        if q.len() > 1 { q.pop_front().flatten() } else { q.front().cloned().flatten() }
    }
    fn secure_input(&self) -> bool {
        self.secure.load(Ordering::SeqCst)
    }
    fn frontmost_app(&self) -> Option<String> {
        Some("FakeApp".into())
    }
    fn snapshot_clipboard(&self) -> ClipboardSnapshot {
        self.clipboard.lock().unwrap().clone()
    }
    fn restore_clipboard(&self, s: ClipboardSnapshot) {
        *self.clipboard.lock().unwrap() = s;
    }
    fn paste(&self, text: &str) -> PasteOutcome {
        *self.clipboard.lock().unwrap() = ClipboardSnapshot::Text(text.into());
        self.pastes.lock().unwrap().push(text.into());
        PasteOutcome::Pasted
    }
    fn accessibility_trusted(&self, _prompt: bool) -> bool {
        true
    }
}
