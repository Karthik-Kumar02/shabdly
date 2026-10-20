//! Windows adapter: UI Automation for the focused field, arboard for the
//! clipboard, enigo for Ctrl+V. No permission prompts are needed.
use super::{ClipboardSnapshot, FieldText, PasteOutcome, Platform};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::hash::{DefaultHasher, Hash, Hasher};
use uiautomation::UIAutomation;
use uiautomation::patterns::UIValuePattern;
use uiautomation::types::ControlType;

/// Control types that can have focus but never take typed text.
const NON_TEXT: &[ControlType] = &[
    ControlType::Button,
    ControlType::CheckBox,
    ControlType::RadioButton,
    ControlType::MenuItem,
    ControlType::Image,
    ControlType::Slider,
];

pub struct WindowsPlatform;

impl WindowsPlatform {
    pub fn new() -> Self {
        WindowsPlatform
    }
}

// ponytail: a UIAutomation client per call; cache one per thread if polling shows up in profiles.
fn automation() -> Option<UIAutomation> {
    // new() fails on threads that already chose a different COM model; COM is usable there anyway.
    UIAutomation::new().or_else(|_| UIAutomation::new_direct()).ok()
}

fn clipboard() -> Option<arboard::Clipboard> {
    arboard::Clipboard::new().ok()
}

fn foreground_process_name() -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(process);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned())
    }
}

impl Platform for WindowsPlatform {
    fn focused_field(&self) -> Option<FieldText> {
        let el = automation()?.get_focused_element().ok()?;
        if el.get_control_type().is_ok_and(|t| NON_TEXT.contains(&t)) {
            return None;
        }
        let value = el.get_pattern::<UIValuePattern>().and_then(|p| p.get_value()).unwrap_or_default();
        let mut h = DefaultHasher::new();
        el.get_runtime_id().unwrap_or_default().hash(&mut h);
        Some(FieldText { value, is_password: el.is_password().unwrap_or(false), element_id: h.finish() })
    }

    fn secure_input(&self) -> bool {
        automation().and_then(|a| a.get_focused_element().ok()).is_some_and(|el| el.is_password().unwrap_or(false))
    }

    fn frontmost_app(&self) -> Option<String> {
        foreground_process_name()
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
            // ponytail: Windows snapshots text or image only; copy every clipboard format if files or rich text get lost.
            ClipboardSnapshot::Items(_) => Ok(()),
        };
    }

    fn paste(&self, text: &str) -> PasteOutcome {
        let Some(mut c) = clipboard() else { return PasteOutcome::Copied };
        if c.set_text(text).is_err() {
            return PasteOutcome::Copied;
        }
        let sent = (|| -> Result<(), Box<dyn std::error::Error>> {
            let mut e = Enigo::new(&Settings::default())?;
            e.key(Key::Control, Direction::Press)?;
            let r = e.key(Key::V, Direction::Click);
            e.key(Key::Control, Direction::Release)?;
            Ok(r?)
        })();
        if sent.is_ok() { PasteOutcome::Pasted } else { PasteOutcome::Copied }
    }

    fn accessibility_trusted(&self, _prompt: bool) -> bool {
        true
    }
}
