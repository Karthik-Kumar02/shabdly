//! Watch the field after insertion until the user is done with it (spec s4).
use crate::platform::Platform;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Poll the focused field. Stops when focus moves to another element, the
/// field is emptied, Enter is pressed, `cancel` is set, or `max` passes.
/// Returns the last non-empty value seen, or None if it was never readable.
pub fn observe(
    platform: &dyn Platform,
    element_id: u64,
    cancel: &AtomicBool,
    enter: &Receiver<()>,
    poll: Duration,
    max: Duration,
) -> Option<String> {
    let start = Instant::now();
    let read = || platform.focused_field().filter(|f| f.element_id == element_id && !f.value.is_empty()).map(|f| f.value);
    let mut last = None;
    loop {
        match read() {
            Some(v) => last = Some(v),
            None => break,
        }
        if cancel.load(Ordering::SeqCst) || start.elapsed() >= max {
            break;
        }
        match enter.recv_timeout(poll) {
            Ok(()) => {
                // One last look: the app may not have sent and cleared the field yet.
                if let Some(v) = read() {
                    last = Some(v);
                }
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(poll),
        }
    }
    last
}
