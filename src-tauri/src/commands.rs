//! Tauri commands for the settings, dictionary and history windows.
use crate::dictation::{Deps, now_ms};
use crate::hotkey::{Hotkey, HotkeyEvent, Tracker};
use crate::engine::EngineSlot;
use crate::local_engine::EngineStatus;
use crate::machine;
use serde::Serialize;
use shabdly_core::asr::WHISPER_MODELS;
use shabdly_core::model_store::{self, ModelState};
use shabdly_core::models::{ModelSpec, best_offered, total_bytes};
use shabdly_core::settings::{LocalEngineKind, ProviderKind, Speed};
use std::path::{Path, PathBuf};
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
    /// The bundled cleanup engine (also inside `deps.provider`), stopped on quit.
    pub local: EngineSlot,
    /// Bundled helper executables (`<resources>/engines`).
    pub engines_dir: PathBuf,
    /// Set while a model download (setup or Speed switch) runs.
    pub downloading: AtomicBool,
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

/// What setup still has to do: "first_run" (never finished) or
/// "speech_model" (the selected Whisper model is missing), or None.
pub fn setup_needed(dir: &std::path::Path, settings: &Settings) -> Option<&'static str> {
    if !dir.join("settings.json").exists() {
        Some("first_run")
    } else if !dir.join("models").join(&settings.whisper_model).exists() {
        Some("speech_model")
    } else {
        None
    }
}

#[tauri::command]
pub fn needs_setup(state: State<AppState>) -> Option<&'static str> {
    let dir = state.deps.data_dir.as_ref()?;
    setup_needed(dir, &state.deps.settings.lock().unwrap())
}

/// macOS "Press Globe key to" setting (AppleFnUsageType): anything but "Do
/// Nothing" (0) makes a Fn tap open a menu or the emoji picker as well.
pub fn globe_key_conflict(usage: Option<&str>) -> bool {
    usage.is_some_and(|u| u.trim() != "0")
}

/// True when the dictation key is Fn and macOS also acts on the Globe key.
#[tauri::command]
pub fn globe_key_warning(state: State<AppState>) -> bool {
    if !cfg!(target_os = "macos") || state.deps.settings.lock().unwrap().hotkey != "Fn" {
        return false;
    }
    let out = std::process::Command::new("defaults").args(["read", "com.apple.HIToolbox", "AppleFnUsageType"]).output();
    let usage = out.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    globe_key_conflict(usage.as_deref())
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

fn models_dir(state: &AppState) -> CmdResult<PathBuf> {
    Ok(state.deps.data_dir.as_ref().ok_or("no data folder")?.join("models"))
}

fn model_path(state: &AppState, name: &str) -> CmdResult<PathBuf> {
    Ok(models_dir(state)?.join(name))
}

#[tauri::command]
pub fn whisper_ready(state: State<AppState>, name: String) -> CmdResult<bool> {
    Ok(model_path(&state, &name)?.exists())
}

#[tauri::command]
pub async fn download_whisper(app: AppHandle, state: State<'_, AppState>, name: String) -> CmdResult<()> {
    let url = WHISPER_MODELS.iter().find(|(n, ..)| *n == name).ok_or("Unknown speech model")?.1;
    let dest = model_path(&state, &name)?;
    setup::download(url, &dest, |done, total| {
        let _ = app.emit("setup-progress", serde_json::json!({ "kind": "whisper", "done": done, "total": total }));
    })
    .await
    .map_err(|e| format!("Download failed: {e:#}"))
}

const GB: u64 = 1_000_000_000;

/// Room for `bytes_needed` plus a 1 GB margin, or the plain message saying how much to free.
pub fn disk_check(bytes_needed: u64, free: u64) -> Result<(), String> {
    match (bytes_needed + GB).saturating_sub(free) {
        0 => Ok(()),
        short => Err(format!("Free up {} GB and try again", short.div_ceil(GB))),
    }
}

/// Clears the download flag when the download ends, however it ends.
pub struct DownloadGuard<'a>(&'a AtomicBool);

impl Drop for DownloadGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// One download at a time: a second Download click or Speed switch is refused, the first goes on.
pub fn start_download(flag: &AtomicBool) -> Result<DownloadGuard<'_>, String> {
    use std::sync::atomic::Ordering::SeqCst;
    flag.compare_exchange(false, true, SeqCst, SeqCst)
        .map(|_| DownloadGuard(flag))
        .map_err(|_| "A download is already running".to_string())
}

/// Bytes still to download for the speech model `name` (0 when it is there).
fn speech_needs(models: &Path, name: &str) -> u64 {
    if models.join(name).exists() {
        return 0;
    }
    WHISPER_MODELS.iter().find(|m| m.0 == name).map_or(0, |m| m.2)
}

/// Bytes `spec` still needs (0 when ready). The first check may hash the files (seconds).
async fn engine_needs(models: PathBuf, spec: &'static ModelSpec) -> CmdResult<u64> {
    let found = tauri::async_runtime::spawn_blocking(move || {
        model_store::locate(spec, &models, model_store::hf_hub_dir().as_deref())
    })
    .await
    .map_err(msg)?
    .map_err(msg)?;
    Ok(match found {
        ModelState::Ready(_) => 0,
        ModelState::Missing { bytes_needed } => bytes_needed,
    })
}

/// Checks free space, then downloads the speech model `whisper` (when given and missing) and then
/// `spec` (when missing), as one `setup-progress` of kind "all". True when `spec` was downloaded.
async fn download_missing(
    app: &AppHandle,
    models: &Path,
    whisper: Option<&str>,
    spec: Option<&'static ModelSpec>,
) -> CmdResult<bool> {
    let speech = whisper.map_or(0, |w| speech_needs(models, w));
    let engine = match spec {
        Some(spec) => engine_needs(models.to_path_buf(), spec).await?,
        None => 0,
    };
    disk_check(speech + engine, machine::free_bytes(models))?;
    // Whole files, so the bar matches what download_model reports for files already there.
    let total = speech + if engine > 0 { spec.map_or(0, total_bytes) } else { 0 };
    let emit = |done: u64| {
        let _ = app.emit("setup-progress", serde_json::json!({ "kind": "all", "done": done, "total": total }));
    };
    let failed = |e: anyhow::Error| {
        eprintln!("model download: {e:#}");
        msg(e)
    };
    if let Some(name) = whisper.filter(|_| speech > 0) {
        let url = WHISPER_MODELS.iter().find(|m| m.0 == name).ok_or("Unknown speech model")?.1;
        setup::download(url, &models.join(name), |done, _| emit(done)).await.map_err(failed)?;
    }
    if let Some(spec) = spec.filter(|_| engine > 0) {
        setup::download_model(spec, models, |done, _| emit(speech + done)).await.map_err(failed)?;
    }
    Ok(engine > 0)
}

/// Starts an engine for `spec` and, only once its own model runs (not a backup), puts it in use
/// and stops the old one. On failure the old engine keeps running.
async fn swap_engine(state: &AppState, spec: &'static ModelSpec) -> CmdResult<()> {
    let data_dir = state.deps.data_dir.as_ref().ok_or("no data folder")?;
    let new = crate::new_engine(data_dir, &state.engines_dir, Some(spec));
    new.ensure().await;
    if new.status() != EngineStatus::Builtin {
        new.stop();
        return Err("The new model did not start, so Shabdly keeps the current one".into());
    }
    let old = std::mem::replace(&mut *state.local.lock().unwrap(), new);
    old.stop();
    Ok(())
}

/// Applies `change` to the current settings and saves them.
fn update_settings(state: &AppState, change: impl FnOnce(&mut Settings)) -> CmdResult<()> {
    let mut current = state.deps.settings.lock().unwrap();
    let mut s = current.clone();
    change(&mut s);
    if let Some(dir) = &state.deps.data_dir {
        store::save(&dir.join("settings.json"), &s).map_err(msg)?;
    }
    *current = s;
    Ok(())
}

fn bundled(s: &Settings) -> bool {
    s.provider == ProviderKind::Local && s.local_engine == LocalEngineKind::Bundled
}

#[derive(Debug, Serialize)]
pub struct ModelsStatus {
    pub speech_ready: bool,
    pub engine_ready: bool,
    /// False on macOS 11.0 to 13.2: no bundled engine, so setup offers Ollama or a cloud engine.
    pub engine_offered: bool,
    /// Bytes still to download for both. 0 for the engine on macOS 11.0 to 13.2 (none offered).
    pub bytes_needed: u64,
    pub free_bytes: u64,
}

#[tauri::command]
pub async fn models_status(state: State<'_, AppState>) -> CmdResult<ModelsStatus> {
    let s = state.deps.settings.lock().unwrap().clone();
    let models = models_dir(&state)?;
    let speech = speech_needs(&models, &s.whisper_model);
    let spec = machine::chosen_spec(&s, &state.engines_dir);
    let engine = match spec {
        Some(spec) => engine_needs(models.clone(), spec).await?,
        None => 0,
    };
    Ok(ModelsStatus {
        speech_ready: models.join(&s.whisper_model).exists(),
        engine_ready: engine == 0,
        engine_offered: spec.is_some(),
        bytes_needed: speech + engine,
        free_bytes: machine::free_bytes(&models),
    })
}

/// The Download step: speech model, then cleanup model, then (when bundled) the engine starts.
#[tauri::command]
pub async fn download_models(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    let _guard = start_download(&state.downloading)?;
    let s = state.deps.settings.lock().unwrap().clone();
    let spec = machine::chosen_spec(&s, &state.engines_dir);
    let fetched = download_missing(&app, &models_dir(&state)?, Some(&s.whisper_model), spec).await?;
    if fetched && bundled(&s) {
        let local = state.local.lock().unwrap().clone();
        local.retry().await;
    }
    Ok(())
}

/// The Best quality / Fastest switch: downloads the other model if missing, then swaps engines.
/// Settings change only when everything worked.
#[tauri::command]
pub async fn set_speed(app: AppHandle, state: State<'_, AppState>, speed: Speed) -> CmdResult<()> {
    let _guard = start_download(&state.downloading)?;
    if speed == Speed::Best && !best_offered(machine::ram_gb(), machine::has_gpu(&state.engines_dir)) {
        return Err("Best quality needs 16 GB of memory or more".into());
    }
    let mut s = state.deps.settings.lock().unwrap().clone();
    s.speed = Some(speed);
    // None: no bundled engine on this macOS, so only the setting is saved.
    if let Some(spec) = machine::chosen_spec(&s, &state.engines_dir) {
        download_missing(&app, &models_dir(&state)?, None, Some(spec)).await?;
        if bundled(&s) {
            swap_engine(&state, spec).await?;
        }
    }
    update_settings(&state, |s| s.speed = Some(speed))
}

#[tauri::command]
pub fn engine_status(state: State<AppState>) -> EngineStatus {
    state.local.lock().unwrap().status()
}

/// Settings "Retry": walk the fallback chain again from step 1.
#[tauri::command]
pub async fn retry_engine(state: State<'_, AppState>) -> CmdResult<()> {
    let local = state.local.lock().unwrap().clone();
    local.retry().await;
    Ok(())
}

/// 0.1.5 upgraders on Ollama: when the cleanup model is on disk and the bundled engine starts,
/// switch to it and return true. Otherwise nothing changes.
#[tauri::command]
pub async fn try_builtin_switch(state: State<'_, AppState>) -> CmdResult<bool> {
    let s = state.deps.settings.lock().unwrap().clone();
    if s.builtin_offer_done || s.provider != ProviderKind::Local {
        return Ok(false);
    }
    let Some(spec) = machine::chosen_spec(&s, &state.engines_dir) else { return Ok(false) };
    if engine_needs(models_dir(&state)?, spec).await? > 0 || swap_engine(&state, spec).await.is_err() {
        return Ok(false);
    }
    update_settings(&state, |s| {
        s.local_engine = LocalEngineKind::Bundled;
        s.builtin_offer_done = true;
    })?;
    Ok(true)
}

#[tauri::command]
pub fn dismiss_builtin_offer(state: State<AppState>) -> CmdResult<()> {
    update_settings(&state, |s| s.builtin_offer_done = true)
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
        if r.start(std::sync::mpsc::channel().0).is_err() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(400));
        // All zeros: macOS is withholding audio (prompt pending or denied).
        r.stop().0.iter().any(|s| *s != 0.0)
    })
    .await
    .unwrap_or(false)
}

/// Open an OS settings page: "accessibility", "input_monitoring", "keyboard"
/// (macOS) or "microphone" (both).
#[tauri::command]
pub fn open_settings_pane(pane: String) -> CmdResult<()> {
    let url = match (pane.as_str(), cfg!(target_os = "macos")) {
        ("accessibility", true) => "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        ("input_monitoring", true) => "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
        ("microphone", true) => "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        ("keyboard", true) => "x-apple.systempreferences:com.apple.Keyboard-Settings.extension",
        ("microphone", false) => "ms-settings:privacy-microphone",
        _ => return Err("unknown settings page".into()),
    };
    let opener = if cfg!(target_os = "macos") { "open" } else { "explorer" };
    std::process::Command::new(opener).arg(url).spawn().map_err(msg)?;
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

    #[test]
    fn setup_needed_when_settings_or_speech_model_missing() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings::default();
        assert_eq!(setup_needed(dir.path(), &s), Some("first_run"));
        std::fs::write(dir.path().join("settings.json"), "{}").unwrap();
        assert_eq!(setup_needed(dir.path(), &s), Some("speech_model"));
        std::fs::create_dir_all(dir.path().join("models")).unwrap();
        std::fs::write(dir.path().join("models").join(&s.whisper_model), "x").unwrap();
        assert_eq!(setup_needed(dir.path(), &s), None);
    }

    #[test]
    fn globe_key_conflict_only_when_set_to_an_action() {
        assert!(!globe_key_conflict(Some("0")));
        assert!(globe_key_conflict(Some("2\n")));
        assert!(globe_key_conflict(Some("1")));
        // Not set: macOS default; Fn dictation worked on the developer's Mac with it.
        assert!(!globe_key_conflict(None));
    }

    #[test]
    fn disk_check_keeps_a_1_gb_margin() {
        assert_eq!(disk_check(5_000_000_000, 5_500_000_000), Err("Free up 1 GB and try again".to_string()));
        assert_eq!(disk_check(5_000_000_000, 7_000_000_000), Ok(()));
        assert_eq!(disk_check(5_000_000_000, 1_000_000_000), Err("Free up 5 GB and try again".to_string()));
    }

    #[test]
    fn second_download_is_refused() {
        let flag = AtomicBool::new(false);
        let first = start_download(&flag).unwrap();
        assert_eq!(start_download(&flag).err(), Some("A download is already running".to_string()));
        drop(first);
        assert!(start_download(&flag).is_ok(), "free again once the first one ends");
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
