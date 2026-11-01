//! OS parts of the one-time move from Shabdly to Voozi: is the old app running, move its API
//! keys, tell the user to quit it. Allowed leftover: this file names the old app on purpose.
use crate::commands::{KEYRING_SERVICE, KEY_PROVIDERS};
use crate::machine::run;
use std::path::Path;
use voozi_core::store::{move_old_data_once, Move};

pub const OLD_ID: &str = "io.github.karthik-kumar02.shabdly";
pub const OLD_KEYRING_SERVICE: &str = "shabdly";

/// True when a `ps -axo comm=` or `tasklist /FO CSV /NH` listing has the old app's process.
fn lists_old_app(process_list: &str) -> bool {
    process_list.lines().any(|line| {
        let first = line.split(',').next().unwrap_or("").trim().trim_matches('"');
        let name = first.rsplit(['/', '\\']).next().unwrap_or("");
        name.eq_ignore_ascii_case("shabdly") || name.eq_ignore_ascii_case("shabdly.exe")
    })
}

fn old_app_running() -> bool {
    #[cfg(target_os = "macos")]
    let out = run(Path::new("/bin/ps"), &["-axo", "comm="]);
    #[cfg(windows)]
    let out = run(
        Path::new("tasklist"),
        &["/FI", "IMAGENAME eq shabdly.exe", "/FO", "CSV", "/NH"],
    );
    #[cfg(not(any(target_os = "macos", windows)))]
    let out: Option<String> = None;
    out.is_some_and(|o| lists_old_app(&o))
}

/// Copy each key to the new service, and delete the old entry only after that write succeeded.
/// Any error leaves both entries as they are. Returns how many moved.
pub fn move_keys(from: &str, to: &str, providers: &[&str]) -> usize {
    providers
        .iter()
        .filter(|p| {
            let (Ok(old), Ok(new)) = (keyring::Entry::new(from, p), keyring::Entry::new(to, p)) else {
                return false;
            };
            let Ok(key) = old.get_password() else { return false };
            new.set_password(&key).is_ok() && old.delete_credential().is_ok()
        })
        .count()
}

fn alert(message: &str) {
    #[cfg(target_os = "macos")]
    {
        run(
            Path::new("/usr/bin/osascript"),
            &[
                "-e",
                "on run argv",
                "-e",
                "display alert \"Voozi\" message (item 1 of argv)",
                "-e",
                "end run",
                message,
            ],
        );
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONWARNING, MB_OK};
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (text, title) = (wide(message), wide("Voozi"));
        // SAFETY: both buffers are NUL-terminated UTF-16 and outlive the call; no owner window.
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONWARNING) };
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    eprintln!("{message}");
}

/// Move the old app's data folder and keys into `data_dir`. True when something was moved.
/// While the old app runs, or when the move fails, tell the user and exit before anything is written.
pub fn move_old_data(data_dir: &Path) -> bool {
    let old = data_dir.parent().unwrap_or(data_dir).join(OLD_ID);
    match move_old_data_once(&old, data_dir, old_app_running) {
        Move::Moved => {
            move_keys(OLD_KEYRING_SERVICE, KEYRING_SERVICE, &KEY_PROVIDERS);
            true
        }
        Move::Skipped => false,
        Move::OldAppRunning => {
            alert("Quit Shabdly, then open Voozi again.");
            std::process::exit(0)
        }
        Move::Failed(e) => {
            alert(&format!("Quit Shabdly, then open Voozi again.\n\n{e}"));
            std::process::exit(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_old_app_on_mac_and_windows() {
        assert!(lists_old_app("/usr/sbin/cfprefsd\n/Applications/Shabdly.app/Contents/MacOS/shabdly\n"));
        assert!(lists_old_app("\"shabdly.exe\",\"4242\",\"Console\",\"1\",\"80,000 K\"\r\n"));
        assert!(lists_old_app("\"Shabdly.exe\",\"4242\",\"Console\",\"1\",\"80,000 K\"\r\n"));
    }

    #[test]
    fn lists_old_app_ignores_others() {
        assert!(!lists_old_app("/Applications/Voozi.app/Contents/MacOS/voozi\n/x/engines/mac/shabdly-engine\n"));
        assert!(!lists_old_app("INFO: No tasks are running which match the specified criteria.\r\n"));
        assert!(!lists_old_app(""));
    }

    #[test]
    fn move_keys_moves_and_deletes_the_old_entry() {
        let tag = getrandom::u64().unwrap();
        let (from, to) = (format!("voozi-test-from-{tag:x}"), format!("voozi-test-to-{tag:x}"));
        keyring::Entry::new(&from, "gemini").unwrap().set_password("k-123").unwrap();
        assert_eq!(move_keys(&from, &to, &["gemini", "anthropic"]), 1);
        let new = keyring::Entry::new(&to, "gemini").unwrap();
        assert_eq!(new.get_password().unwrap(), "k-123");
        assert!(matches!(keyring::Entry::new(&from, "gemini").unwrap().get_password(), Err(keyring::Error::NoEntry)));
        new.delete_credential().unwrap();
    }
}
