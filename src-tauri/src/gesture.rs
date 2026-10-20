//! Hotkey gestures (spec s1): hold to talk, double-tap to lock, Esc cancels.
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Down(Instant),
    Up(Instant),
    Esc,
    Tick(Instant),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Cancel,
    None,
}

pub const MIN_HOLD: Duration = Duration::from_millis(300);
pub const DOUBLE_TAP: Duration = Duration::from_millis(400);
pub const MAX_LEN: Duration = Duration::from_secs(6 * 60);

#[derive(Debug, Clone, Copy, Default)]
enum State {
    #[default]
    Idle,
    Holding(Instant),
    TapPending(Instant),
    Locked(Instant),
}

#[derive(Debug, Default)]
pub struct Gesture {
    state: State,
}

impl Gesture {
    pub fn feed(&mut self, input: Input) -> Action {
        use State::*;
        let (next, action) = match (self.state, input) {
            (Idle, Input::Down(t)) => (Holding(t), Action::Start),
            (TapPending(up), Input::Down(t)) if t.duration_since(up) <= DOUBLE_TAP => (Locked(t), Action::Start),
            (TapPending(_), Input::Down(t)) => (Holding(t), Action::Start),
            (Holding(start), Input::Up(t)) if t.duration_since(start) >= MIN_HOLD => (Idle, Action::Stop),
            (Holding(_), Input::Up(t)) => (TapPending(t), Action::Cancel),
            (Locked(_), Input::Down(_)) => (Idle, Action::Stop),
            (Holding(_) | Locked(_), Input::Esc) => (Idle, Action::Cancel),
            (Holding(start) | Locked(start), Input::Tick(t)) if t.duration_since(start) > MAX_LEN => (Idle, Action::Stop),
            (state, _) => (state, Action::None),
        };
        self.state = next;
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn hold_and_release() {
        let t = Instant::now();
        let mut g = Gesture::default();
        assert_eq!(g.feed(Input::Down(t)), Action::Start);
        assert_eq!(g.feed(Input::Up(ms(t, 1200))), Action::Stop);
        assert_eq!(g.feed(Input::Up(ms(t, 1300))), Action::None);
    }

    #[test]
    fn short_tap_cancels() {
        let t = Instant::now();
        let mut g = Gesture::default();
        assert_eq!(g.feed(Input::Down(t)), Action::Start);
        assert_eq!(g.feed(Input::Up(ms(t, 100))), Action::Cancel);
    }

    #[test]
    fn double_tap_locks_then_tap_stops() {
        let t = Instant::now();
        let mut g = Gesture::default();
        g.feed(Input::Down(t));
        assert_eq!(g.feed(Input::Up(ms(t, 100))), Action::Cancel);
        assert_eq!(g.feed(Input::Down(ms(t, 250))), Action::Start);
        assert_eq!(g.feed(Input::Up(ms(t, 330))), Action::None);
        assert_eq!(g.feed(Input::Tick(ms(t, 5000))), Action::None);
        assert_eq!(g.feed(Input::Down(ms(t, 9000))), Action::Stop);
        assert_eq!(g.feed(Input::Up(ms(t, 9080))), Action::None);
    }

    #[test]
    fn slow_second_tap_is_new_hold() {
        let t = Instant::now();
        let mut g = Gesture::default();
        g.feed(Input::Down(t));
        g.feed(Input::Up(ms(t, 100)));
        assert_eq!(g.feed(Input::Down(ms(t, 900))), Action::Start);
        assert_eq!(g.feed(Input::Up(ms(t, 2000))), Action::Stop);
    }

    #[test]
    fn esc_cancels_hold_and_locked() {
        let t = Instant::now();
        let mut g = Gesture::default();
        g.feed(Input::Down(t));
        assert_eq!(g.feed(Input::Esc), Action::Cancel);
        assert_eq!(g.feed(Input::Up(ms(t, 800))), Action::None);
        assert_eq!(g.feed(Input::Esc), Action::None);

        g.feed(Input::Down(ms(t, 2000)));
        g.feed(Input::Up(ms(t, 2100)));
        assert_eq!(g.feed(Input::Down(ms(t, 2200))), Action::Start);
        g.feed(Input::Up(ms(t, 2300)));
        assert_eq!(g.feed(Input::Esc), Action::Cancel);
        assert_eq!(g.feed(Input::Down(ms(t, 4000))), Action::Start);
    }

    #[test]
    fn six_minute_cap_stops() {
        let t = Instant::now();
        let mut g = Gesture::default();
        g.feed(Input::Down(t));
        assert_eq!(g.feed(Input::Tick(ms(t, 359_000))), Action::None);
        assert_eq!(g.feed(Input::Tick(ms(t, 360_001))), Action::Stop);
        assert_eq!(g.feed(Input::Up(ms(t, 361_000))), Action::None);
    }
}
