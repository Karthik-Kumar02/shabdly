//! Tauri commands for the settings, dictionary and history windows.
use crate::dictation::{Deps, now_ms};
use crate::hotkey::{Hotkey, HotkeyEvent, Tracker};
use shabdly_core::asr::WHISPER_MODELS;
use shabdly_core::setup::{self, OllamaStatus};
use std::sync::mpsc::Sender;
use std::time::Duration;
use tauri::Emitter;
use shabdly_core::dictionary::Entry;
use shabdly_core::history::HistoryItem;
use shabdly_core::learning::{LearnEvent, learn, undo};
use shabdly_core::settings::Settings;
use shabdly_core::store;
use std::sync::atomic::{AtomicBool, AtomicI64};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, State};

pub const KEYRING_SERVICE: &str = "shabdly";
const KEY_PROVIDERS: [&str; 2] = ["gemini", "anthropic"];

pub struct AppState {
    pub deps: Arc<Deps>,
    pub hotkey: Mutex<Option<Hotkey>>,
    pub warm: Arc<AtomicBool>,
    /// Unix ms of the last hotkey event the listener delivered (0 = none yet).
    pub last_hotkey_ms: Arc<AtomicI64>,
    /// Where a (re)started hotkey listener sends its events.
    pub hotkey_tx: Sender<HotkeyEvent>,
}

type CmdResult<T> = Result<T, String>;

fn msg(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// "Fix a word" on a history item: learn `wrong` -> `right` with the usual rules.
pub fn fix_word(deps: &Deps, at_ms: i64, wrong: &str, right: &str) -> anyhow::Result<Vec<LearnEvent>> {
    if !deps.history.lock().unwrap().items.iter().any(|i| i.at_ms == at_ms) {
        anyhow::bail!("That history item is no longer there");
    }
    let events = learn(&mut deps.dict.lock().unwrap(), wrong.trim(), right.trim(), now_ms() / 1000);
    deps.save_dict();
    Ok(events)
}

#[tauri::command]
pub fn get_settings(state: State<AppState>) -> Settings {
    state.deps.settings.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_settings(state: State<AppState>, settings: Settings) -> CmdResult<()> {
    Tracker::new(&settings.hotkey).map_err(msg)?;
    let mut hk = state.hotkey.lock().unwrap();
    match hk.as_ref() {
        Some(h) => h.set_key(&settings.hotkey).map_err(msg)?,
        None => *hk = crate::hotkey::spawn(settings.hotkey.clone(), state.hotkey_tx.clone()).ok(),
    }
    drop(hk);
    if let Some(dir) = &state.deps.data_dir {
        store::save(&dir.join("settings.json"), &settings).map_err(msg)?;
    }
    *state.deps.settings.lock().unwrap() = settings;
    Ok(())
}

#[tauri::command]
pub fn list_dictionary(state: State<AppState>) -> Vec<Entry> {
    state.deps.dict.lock().unwrap().entries.clone()
}

fn clean_list(xs: Vec<String>) -> Vec<String> {
    xs.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

#[tauri::command]
pub fn upsert_entry(
    state: State<AppState>,
    word: String,
    heard_as: Vec<String>,
    replace_with: Option<String>,
) -> CmdResult<()> {
    if word.trim().is_empty() {
        return Err("Type the word first".into());
    }
    let replace_with = replace_with.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    state.deps.dict.lock().unwrap().upsert_manual(&word, clean_list(heard_as), replace_with, now_ms() / 1000);
    state.deps.save_dict();
    Ok(())
}

#[tauri::command]
pub fn delete_entry(state: State<AppState>, word: String) {
    state.deps.dict.lock().unwrap().remove(&word);
    state.deps.save_dict();
}

#[tauri::command]
pub fn star_entry(state: State<AppState>, word: String, starred: bool) {
    state.deps.dict.lock().unwrap().set_starred(&word, starred);
    state.deps.save_dict();
}

#[tauri::command]
pub fn import_dictionary(state: State<AppState>, json: String) -> CmdResult<usize> {
    let added = state.deps.dict.lock().unwrap().import_json(&json).map_err(|e| format!("Not a Shabdly dictionary: {e}"))?;
    state.deps.save_dict();
    Ok(added)
}

#[tauri::command]
pub fn export_dictionary(state: State<AppState>) -> String {
    state.deps.dict.lock().unwrap().export_json()
}

#[tauri::command]
pub fn list_history(state: State<AppState>) -> Vec<HistoryItem> {
    state.deps.history.lock().unwrap().items.iter().rev().cloned().collect()
}

#[tauri::command]
pub fn clear_history(state: State<AppState>) {
    state.deps.history.lock().unwrap().clear();
    state.deps.save_history();
}

#[tauri::command(rename_all = "camelCase")]
pub fn fix_word_cmd(state: State<AppState>, at_ms: i64, wrong: String, right: String) -> CmdResult<Vec<LearnEvent>> {
    fix_word(&state.deps, at_ms, &wrong, &right).map_err(msg)
}

#[tauri::command]
pub fn undo_learn(state: State<AppState>, event: LearnEvent) {
    undo(&mut state.deps.dict.lock().unwrap(), &event);
    state.deps.save_dict();
}

fn key_entry(provider: &str) -> CmdResult<keyring::Entry> {
    if !KEY_PROVIDERS.contains(&provider) {
        return Err(format!("Unknown provider {provider}"));
    }
    keyring::Entry::new(KEYRING_SERVICE, provider).map_err(msg)
}

/// Read a cloud API key from the OS credential store.
pub fn api_key(provider: &str) -> Option<String> {
    key_entry(provider).ok()?.get_password().ok().filter(|k| !k.is_empty())
}

/// Save (or, when empty, delete) a cloud API key. Keys never touch files or logs.
#[tauri::command]
pub fn set_api_key(provider: String, key: String) -> CmdResult<()> {
    let entry = key_entry(&provider)?;
    let key = key.trim();
    if key.is_empty() {
        return match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(msg(e)),
        };
    }
    entry.set_password(key).map_err(|_| "Could not save the key to the system keychain".to_string())
}

#[tauri::command]
pub fn has_api_key(provider: String) -> bool {
    api_key(&provider).is_some()
}

/// Paste a history item into the app that was in front before Shabdly's window.
#[tauri::command]
pub async fn reinsert(app: AppHandle, state: State<'_, AppState>, text: String) -> CmdResult<()> {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    let platform = state.deps.platform.clone();
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        crate::platform::insert(platform.as_ref(), &text);
    })
    .await
    .map_err(msg)
}

/// The overlay ignores the mouse except while a toast with Undo is showing.
#[tauri::command]
pub fn overlay_interactive(app: AppHandle, on: bool) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.set_ignore_cursor_events(!on);
    }
}

const OLLAMA_URL: &str = "http://localhost:11434";
const LOCAL_MODEL: &str = "gemma4:e4b";

/// True when settings.json does not exist yet: first-run setup has not finished.
#[tauri::command]
pub fn needs_setup(state: State<AppState>) -> bool {
    state.deps.data_dir.as_ref().is_some_and(|d| !d.join("settings.json").exists())
}

#[tauri::command]
pub async fn ollama_status() -> OllamaStatus {
    setup::ollama_status(OLLAMA_URL, LOCAL_MODEL).await
}

#[tauri::command]
pub async fn pull_model(app: AppHandle) -> CmdResult<()> {
    setup::pull(OLLAMA_URL, LOCAL_MODEL, |done, total| {
        let _ = app.emit("setup-progress", serde_json::json!({ "kind": "model", "done": done, "total": total }));
    })
    .await
    .map_err(|e| format!("{e:#}"))
}

fn model_path(state: &AppState, name: &str) -> CmdResult<std::path::PathBuf> {
    let dir = state.deps.data_dir.as_ref().ok_or("no data folder")?;
    Ok(dir.join("models").join(name))
}

#[tauri::command]
pub fn whisper_ready(state: State<AppState>, name: String) -> CmdResult<bool> {
    Ok(model_path(&state, &name)?.exists())
}

#[tauri::command]
pub async fn download_whisper(app: AppHandle, state: State<'_, AppState>, name: String) -> CmdResult<()> {
    let url = WHISPER_MODELS.iter().find(|(n, _)| *n == name).ok_or("Unknown speech model")?.1;
    let dest = model_path(&state, &name)?;
    setup::download(url, &dest, |done, total| {
        let _ = app.emit("setup-progress", serde_json::json!({ "kind": "whisper", "done": done, "total": total }));
    })
    .await
    .map_err(|e| format!("Download failed: {e:#}"))
}

/// Shows the macOS Accessibility prompt when not yet allowed.
#[tauri::command]
pub fn request_accessibility(state: State<AppState>) -> bool {
    state.deps.platform.accessibility_trusted(true)
}

#[tauri::command]
pub fn accessibility_status(state: State<AppState>) -> bool {
    state.deps.platform.accessibility_trusted(false)
}

/// Open the mic briefly so the OS asks for permission. True when it opened.
#[tauri::command]
pub async fn request_microphone() -> bool {
    tauri::async_runtime::spawn_blocking(|| {
        let mut r = crate::capture::Recorder::default();
        let ok = r.start(std::sync::mpsc::channel().0).is_ok();
        std::thread::sleep(Duration::from_millis(300));
        r.stop();
        ok
    })
    .await
    .unwrap_or(false)
}

/// Open a privacy page in System Settings (macOS); `pane` is
/// "accessibility", "input_monitoring" or "microphone".
#[tauri::command]
pub fn open_settings_pane(pane: String) -> CmdResult<()> {
    let anchor = match pane.as_str() {
        "accessibility" => "Privacy_Accessibility",
        "input_monitoring" => "Privacy_ListenEvent",
        "microphone" => "Privacy_Microphone",
        _ => return Err("unknown settings page".into()),
    };
    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(format!("x-apple.systempreferences:com.apple.preference.security?{anchor}"))
        .spawn()
        .map_err(msg)?;
    #[cfg(not(target_os = "macos"))]
    let _ = anchor;
    Ok(())
}

/// Wait up to `wait` for a hotkey event that arrives after the call starts.
pub async fn wait_for_hotkey(last_ms: &AtomicI64, wait: Duration) -> bool {
    let start = now_ms();
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        if last_ms.load(std::sync::atomic::Ordering::SeqCst) >= start {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// True when the dictation key reached Shabdly within `ms`. Starts the
/// listener first if it could not start before (permission granted since).
#[tauri::command]
pub async fn hotkey_health(state: State<'_, AppState>, ms: u64) -> CmdResult<bool> {
    {
        let mut hk = state.hotkey.lock().unwrap();
        if hk.is_none() {
            let key = state.deps.settings.lock().unwrap().hotkey.clone();
            *hk = crate::hotkey::spawn(key, state.hotkey_tx.clone()).ok();
        }
    }
    Ok(wait_for_hotkey(&state.last_hotkey_ms, Duration::from_millis(ms)).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictation::tests::{Echo, deps};
    use crate::platform::fake::FakePlatform;
    use shabdly_core::history::HistoryItem;
    use std::sync::Arc;

    #[test]
    fn fix_word_adds_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = deps(Arc::new(FakePlatform::new(vec![None])), "", Arc::new(Echo));
        d.data_dir = Some(dir.path().to_path_buf());
        d.history.lock().unwrap().push(HistoryItem {
            at_ms: 5,
            raw: "ask kartik now".into(),
            cleaned: "ask kartik now".into(),
            final_text: None,
            app: None,
        });
        let events = fix_word(&d, 5, "kartik", "Karthik").unwrap();
        assert_eq!(events, vec![LearnEvent::Added { word: "Karthik".into(), heard: "kartik".into() }]);
        assert_eq!(d.dict.lock().unwrap().get("Karthik").unwrap().heard_as, vec!["kartik"]);
        assert!(std::fs::read_to_string(dir.path().join("dictionary.json")).unwrap().contains("Karthik"));
        assert!(fix_word(&d, 99, "a", "b").is_err(), "unknown history item");
    }

    #[tokio::test]
    async fn hotkey_health_reports_silence() {
        let last = Arc::new(AtomicI64::new(0));
        assert!(!wait_for_hotkey(&last, std::time::Duration::from_millis(50)).await);

        // An event during the wait counts; one from before the wait does not.
        last.store(now_ms() - 60_000, std::sync::atomic::Ordering::SeqCst);
        let l2 = last.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            l2.store(now_ms(), std::sync::atomic::Ordering::SeqCst);
        });
        assert!(wait_for_hotkey(&last, std::time::Duration::from_millis(500)).await);
    }
}
