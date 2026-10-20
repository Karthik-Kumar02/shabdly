pub mod capture;
pub mod commands;
pub mod dictation;
pub mod engine;
pub mod gesture;
pub mod hotkey;
pub mod observe;
pub mod platform;

use commands::AppState;
use dictation::{Controller, Deps, Emit, now_ms};
use engine::{LazyAsr, SwitchProvider};
use serde::de::DeserializeOwned;
use shabdly_core::pipeline::{unload, warm_up};
use shabdly_core::providers::OllamaProvider;
use shabdly_core::settings::{ProviderKind, Settings};
use shabdly_core::store;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, RunEvent, WindowEvent};

const OVERLAY_W: f64 = 240.0;
const OVERLAY_H: f64 = 48.0;
const OVERLAY_BOTTOM_GAP: f64 = 80.0;

/// A corrupt file must not stop the app; fall back to defaults and say so.
fn load_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    store::load(path).unwrap_or_else(|e| {
        eprintln!("{e:#}; starting fresh");
        T::default()
    })
}

/// Bottom center of the monitor under the mouse, 80 px above its bottom edge.
fn place_overlay(app: &AppHandle) {
    let Some(w) = app.get_webview_window("overlay") else { return };
    let Ok(p) = app.cursor_position() else { return };
    let Ok(Some(m)) = app.monitor_from_point(p.x, p.y) else { return };
    let s = m.scale_factor();
    let (pos, size) = (m.position(), m.size());
    let x = pos.x as f64 + (size.width as f64 - OVERLAY_W * s) / 2.0;
    let y = pos.y as f64 + size.height as f64 - (OVERLAY_H + OVERLAY_BOTTOM_GAP) * s;
    let _ = w.set_size(LogicalSize::new(OVERLAY_W, OVERLAY_H));
    let _ = w.set_position(PhysicalPosition::new(x, y));
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory);

    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(data_dir.join("models"))?;
    let first_run = !data_dir.join("settings.json").exists();
    let settings: Settings = load_or_default(&data_dir.join("settings.json"));
    let settings = Arc::new(Mutex::new(settings));
    let asr = Arc::new(LazyAsr::new(data_dir.join("models"), settings.clone()));
    let deps = Arc::new(Deps {
        platform: platform::current(),
        asr: asr.clone(),
        provider: Arc::new(SwitchProvider { settings: settings.clone() }),
        dict: Arc::new(Mutex::new(load_or_default(&data_dir.join("dictionary.json")))),
        history: Arc::new(Mutex::new(load_or_default(&data_dir.join("history.json")))),
        settings: settings.clone(),
        data_dir: Some(data_dir.clone()),
        poll: Duration::from_millis(250),
        max_observe: Duration::from_secs(60),
    });

    // Every event goes to the windows; a recording start also moves the overlay
    // to the screen the user is working on.
    let handle = app.handle().clone();
    let emit: Emit = Arc::new(move |name, payload| {
        if name == "overlay-state" && payload["state"] == "recording" && payload.get("level").is_none() {
            place_overlay(&handle);
        }
        let _ = handle.emit(name, payload);
    });

    // Hotkey events pass through here so first-run setup can check the key works.
    let (raw_tx, raw_rx) = channel();
    let (ctl_tx, ctl_rx) = channel();
    let last_hotkey_ms = Arc::new(AtomicI64::new(0));
    let seen = last_hotkey_ms.clone();
    std::thread::spawn(move || {
        for ev in raw_rx {
            seen.store(now_ms(), Ordering::SeqCst);
            if ctl_tx.send(ev).is_err() {
                return;
            }
        }
    });
    let key = settings.lock().unwrap().hotkey.clone();
    let hotkey = hotkey::spawn(key, raw_tx)
        .map_err(|e| eprintln!("hotkey listener not started: {e:#}"))
        .ok();

    let warm = Arc::new(AtomicBool::new(false));
    Controller::spawn(deps.clone(), ctl_rx, emit, warm.clone(), tauri::async_runtime::handle().inner().clone());

    // Keep the local model loaded so dictations are fast (spec s6).
    let local = settings.lock().unwrap().provider == ProviderKind::Local;
    let w = warm.clone();
    tauri::async_runtime::spawn(async move {
        if local {
            let _ = warm_up(&OllamaProvider::default()).await;
        }
        w.store(true, Ordering::SeqCst);
    });
    std::thread::spawn(move || asr.preload());

    if let Some(o) = app.get_webview_window("overlay") {
        let _ = o.set_ignore_cursor_events(true);
        place_overlay(app.handle());
        let _ = o.show();
    }

    let open = MenuItem::with_id(app, "open", "Open Shabdly", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Shabdly", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::new().menu(&menu).tooltip("Shabdly").on_menu_event(|app, e| match e.id.as_ref() {
        "open" => show_main(app),
        "quit" => app.exit(0),
        _ => {}
    });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;

    app.manage(AppState { deps, hotkey: Mutex::new(hotkey), warm, last_hotkey_ms, first_run });
    if first_run {
        show_main(app.handle());
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(setup)
        .on_window_event(|w, e| {
            // Closing the settings window only hides it; Shabdly keeps running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = e {
                if w.label() == "main" {
                    api.prevent_close();
                    let _ = w.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::set_settings,
            commands::list_dictionary,
            commands::upsert_entry,
            commands::delete_entry,
            commands::star_entry,
            commands::import_dictionary,
            commands::export_dictionary,
            commands::list_history,
            commands::clear_history,
            commands::fix_word_cmd,
            commands::undo_learn,
            commands::set_api_key,
            commands::has_api_key,
            commands::reinsert,
            commands::overlay_interactive,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Shabdly");
    app.run(|_, event| {
        if let RunEvent::Exit = event {
            // Free the local model's memory; never block quitting for long.
            tauri::async_runtime::block_on(async {
                let _ = tokio::time::timeout(Duration::from_secs(1), unload(&OllamaProvider::default())).await;
            });
        }
    });
}
