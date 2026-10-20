//! Global hotkey listener. macOS uses a listen-only CoreGraphics event tap;
//! Windows uses rdev. Both feed raw key changes into one shared `Tracker`.
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Down,
    Up,
    Esc,
    Enter,
}

/// Physical keys the listeners report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raw {
    Fn,
    CtrlL,
    CtrlR,
    AltL,
    AltR,
    ShiftL,
    ShiftR,
    MetaL,
    MetaR,
    F(u8),
    Esc,
    Enter,
}

/// What a user can bind. `Ctrl` matches either side; `RightAlt` only the right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Fn,
    Ctrl,
    RightCtrl,
    Alt,
    RightAlt,
    Shift,
    RightShift,
    Meta,
    RightMeta,
    F(u8),
}

impl Key {
    fn parse(name: &str) -> Option<Key> {
        let n = name.trim().to_lowercase().replace([' ', '_', '-'], "");
        Some(match n.as_str() {
            "fn" | "globe" => Key::Fn,
            "ctrl" | "control" => Key::Ctrl,
            "rightctrl" | "rightcontrol" => Key::RightCtrl,
            "alt" | "option" => Key::Alt,
            "rightalt" | "rightoption" | "altgr" => Key::RightAlt,
            "shift" => Key::Shift,
            "rightshift" => Key::RightShift,
            "win" | "cmd" | "command" | "meta" | "super" => Key::Meta,
            "rightwin" | "rightcmd" | "rightcommand" => Key::RightMeta,
            f if f.starts_with('f') => Key::F(f[1..].parse().ok().filter(|n| (1..=24).contains(n))?),
            _ => return None,
        })
    }

    fn matches(self, r: Raw) -> bool {
        use Raw::*;
        match self {
            Key::Fn => r == Fn,
            Key::Ctrl => matches!(r, CtrlL | CtrlR),
            Key::RightCtrl => r == CtrlR,
            Key::Alt => matches!(r, AltL | AltR),
            Key::RightAlt => r == AltR,
            Key::Shift => matches!(r, ShiftL | ShiftR),
            Key::RightShift => r == ShiftR,
            Key::Meta => matches!(r, MetaL | MetaR),
            Key::RightMeta => r == MetaR,
            Key::F(n) => r == F(n),
        }
    }
}

/// Turns raw key changes into hotkey Down/Up edges (auto-repeat ignored).
pub struct Tracker {
    combo: Vec<Key>,
    down: Vec<Raw>,
    active: bool,
}

impl Tracker {
    pub fn new(name: &str) -> anyhow::Result<Tracker> {
        let combo = name
            .split('+')
            .map(|p| Key::parse(p).ok_or_else(|| anyhow::anyhow!("unknown key {p:?}")))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Tracker { combo, down: vec![], active: false })
    }

    pub fn handle(&mut self, raw: Raw, pressed: bool) -> Option<HotkeyEvent> {
        match (raw, pressed) {
            (Raw::Esc, true) => return Some(HotkeyEvent::Esc),
            (Raw::Enter, true) => return Some(HotkeyEvent::Enter),
            (_, true) if !self.down.contains(&raw) => self.down.push(raw),
            (_, false) => self.down.retain(|r| *r != raw),
            _ => {}
        }
        let all = self.combo.iter().all(|k| self.down.iter().any(|r| k.matches(*r)));
        match (self.active, all) {
            (false, true) => {
                self.active = true;
                Some(HotkeyEvent::Down)
            }
            (true, false) => {
                self.active = false;
                Some(HotkeyEvent::Up)
            }
            _ => None,
        }
    }
}

/// Handle to the running listener; the bound key can change at runtime.
#[derive(Clone)]
pub struct Hotkey {
    tracker: Arc<Mutex<Tracker>>,
}

impl Hotkey {
    pub fn set_key(&self, name: &str) -> anyhow::Result<()> {
        *self.tracker.lock().unwrap() = Tracker::new(name)?;
        Ok(())
    }
}

/// Start listening on its own thread. Fails when the key name is unknown or
/// the OS refuses the listener (macOS without Input Monitoring).
pub fn spawn(key: String, tx: Sender<HotkeyEvent>) -> anyhow::Result<Hotkey> {
    let tracker = Arc::new(Mutex::new(Tracker::new(&key)?));
    let feed = {
        let tracker = tracker.clone();
        move |raw: Raw, pressed: bool| {
            if let Some(ev) = tracker.lock().unwrap().handle(raw, pressed) {
                let _ = tx.send(ev);
            }
        }
    };
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("hotkey".into()).spawn(move || backend::listen(feed, ready_tx))?;
    ready_rx.recv().map_err(|_| anyhow::anyhow!("hotkey listener exited"))??;
    Ok(Hotkey { tracker })
}

#[cfg(target_os = "macos")]
mod backend {
    use super::Raw;
    use core_foundation::runloop::CFRunLoop;
    use core_graphics::event::{
        CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType, CallbackResult,
        EventField,
    };
    use std::sync::mpsc::Sender;

    fn key_raw(code: i64) -> Option<Raw> {
        Some(match code {
            53 => Raw::Esc,
            36 | 76 => Raw::Enter,
            122 => Raw::F(1),
            120 => Raw::F(2),
            99 => Raw::F(3),
            118 => Raw::F(4),
            96 => Raw::F(5),
            97 => Raw::F(6),
            98 => Raw::F(7),
            100 => Raw::F(8),
            101 => Raw::F(9),
            109 => Raw::F(10),
            103 => Raw::F(11),
            111 => Raw::F(12),
            105 => Raw::F(13),
            107 => Raw::F(14),
            113 => Raw::F(15),
            106 => Raw::F(16),
            64 => Raw::F(17),
            79 => Raw::F(18),
            80 => Raw::F(19),
            90 => Raw::F(20),
            _ => return None,
        })
    }

    /// Modifier keycode and its device-specific flag bit (IOLLEvent.h).
    fn modifier(code: i64) -> Option<(Raw, u64)> {
        Some(match code {
            63 => (Raw::Fn, 0x0080_0000),
            59 => (Raw::CtrlL, 0x0001),
            62 => (Raw::CtrlR, 0x2000),
            58 => (Raw::AltL, 0x0020),
            61 => (Raw::AltR, 0x0040),
            56 => (Raw::ShiftL, 0x0002),
            60 => (Raw::ShiftR, 0x0004),
            55 => (Raw::MetaL, 0x0008),
            54 => (Raw::MetaR, 0x0010),
            _ => return None,
        })
    }

    pub fn listen(feed: impl Fn(Raw, bool) + 'static, ready: Sender<anyhow::Result<()>>) {
        let ready2 = ready.clone();
        let res = CGEventTap::with_enabled(
            CGEventTapLocation::Session,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            vec![CGEventType::KeyDown, CGEventType::KeyUp, CGEventType::FlagsChanged],
            move |_, ty, ev| {
                let code = ev.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
                match ty {
                    CGEventType::KeyDown | CGEventType::KeyUp => {
                        if let Some(raw) = key_raw(code) {
                            feed(raw, matches!(ty, CGEventType::KeyDown));
                        }
                    }
                    CGEventType::FlagsChanged => {
                        if let Some((raw, bit)) = modifier(code) {
                            feed(raw, ev.get_flags().bits() & bit != 0);
                        }
                    }
                    _ => {}
                }
                CallbackResult::Keep
            },
            move || {
                let _ = ready2.send(Ok(()));
                CFRunLoop::run_current();
            },
        );
        if res.is_err() {
            let _ = ready.send(Err(anyhow::anyhow!("macOS refused the key listener (Input Monitoring not granted)")));
        }
    }
}

#[cfg(target_os = "windows")]
mod backend {
    use super::Raw;
    use rdev::{EventType, Key};
    use std::sync::mpsc::Sender;

    fn raw(k: Key) -> Option<Raw> {
        Some(match k {
            Key::Function => Raw::Fn,
            Key::ControlLeft => Raw::CtrlL,
            Key::ControlRight => Raw::CtrlR,
            Key::Alt => Raw::AltL,
            Key::AltGr => Raw::AltR,
            Key::ShiftLeft => Raw::ShiftL,
            Key::ShiftRight => Raw::ShiftR,
            Key::MetaLeft => Raw::MetaL,
            Key::MetaRight => Raw::MetaR,
            Key::Escape => Raw::Esc,
            Key::Return | Key::KpReturn => Raw::Enter,
            Key::F1 => Raw::F(1),
            Key::F2 => Raw::F(2),
            Key::F3 => Raw::F(3),
            Key::F4 => Raw::F(4),
            Key::F5 => Raw::F(5),
            Key::F6 => Raw::F(6),
            Key::F7 => Raw::F(7),
            Key::F8 => Raw::F(8),
            Key::F9 => Raw::F(9),
            Key::F10 => Raw::F(10),
            Key::F11 => Raw::F(11),
            Key::F12 => Raw::F(12),
            _ => return None,
        })
    }

    pub fn listen(feed: impl Fn(Raw, bool) + 'static, ready: Sender<anyhow::Result<()>>) {
        let _ = ready.send(Ok(()));
        let _ = rdev::listen(move |e| match e.event_type {
            EventType::KeyPress(k) => raw(k).into_iter().for_each(|r| feed(r, true)),
            EventType::KeyRelease(k) => raw(k).into_iter().for_each(|r| feed(r, false)),
            _ => {}
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use HotkeyEvent::*;

    #[test]
    fn single_key_hold() {
        let mut t = Tracker::new("Fn").unwrap();
        assert_eq!(t.handle(Raw::Fn, true), Some(Down));
        assert_eq!(t.handle(Raw::Fn, true), None, "auto-repeat is not a new press");
        assert_eq!(t.handle(Raw::CtrlL, true), None);
        assert_eq!(t.handle(Raw::Fn, false), Some(Up));
    }

    #[test]
    fn combo_needs_all_keys_and_either_side() {
        let mut t = Tracker::new("Ctrl+Win").unwrap();
        assert_eq!(t.handle(Raw::CtrlR, true), None);
        assert_eq!(t.handle(Raw::MetaL, true), Some(Down));
        assert_eq!(t.handle(Raw::CtrlR, false), Some(Up));
        assert_eq!(t.handle(Raw::MetaL, false), None);
    }

    #[test]
    fn right_alt_ignores_left_alt() {
        let mut t = Tracker::new("RightAlt").unwrap();
        assert_eq!(t.handle(Raw::AltL, true), None);
        assert_eq!(t.handle(Raw::AltR, true), Some(Down));
    }

    #[test]
    fn esc_and_enter_always_reported() {
        let mut t = Tracker::new("F13").unwrap();
        assert_eq!(t.handle(Raw::Esc, true), Some(Esc));
        assert_eq!(t.handle(Raw::Enter, true), Some(Enter));
        assert_eq!(t.handle(Raw::Enter, false), None);
        assert_eq!(t.handle(Raw::F(13), true), Some(Down));
    }

    #[test]
    fn unknown_key_name_is_error() {
        assert!(Tracker::new("Banana").is_err());
        assert!(Tracker::new("").is_err());
        assert!(Tracker::new("cmd + shift").is_ok());
    }
}
