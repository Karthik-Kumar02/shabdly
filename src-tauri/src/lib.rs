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
use shabdly_core::pipeline::{unload, warm_up};
use shabdly_core::providers::OllamaProvider;
use shabdly_core::settings::{ProviderKind, Settings};
use shabdly_core::store;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, RunEvent, WindowEvent};

const OVERLAY_W: f64 = 240.0;
const OVERLAY_H: f64 = 48.0;
const OVERLAY_BOTTOM_GAP: f64 = 80.0;

/// A monitor as (x, y, width, height, scale), all in physical pixels.
type MonitorRect = (i32, i32, u32, u32, f64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_goes_bottom_center_of_monitor_under_cursor() {
        let mons = [(0, 0, 3024, 1964, 2.0), (3024, 0, 1920, 1080, 1.0)];
        // Retina screen: 240x48 logical is 480x96 physical, 80 logical above the bottom.
        assert_eq!(overlay_position(&mons, (1679.0, 1420.0)), Some((1272.0, 1964.0 - 96.0 - 160.0)));
        assert_eq!(overlay_position(&mons, (3500.0, 10.0)), Some((3024.0 + 840.0, 1080.0 - 48.0 - 80.0)));
        // Cursor outside every monitor: use the first one.
        assert_eq!(overlay_position(&mons, (-5.0, -5.0)), Some((1272.0, 1708.0)));
        assert_eq!(overlay_position(&[], (0.0, 0.0)), None);
    }
}

/// Bottom center of the monitor under the cursor (physical pixels), 80 px
/// above its bottom edge; the first monitor when the cursor is on none.
fn overlay_position(monitors: &[MonitorRect], cursor: (f64, f64)) -> Option<(f64, f64)> {
    let inside = |&&(x, y, w, h, _): &&MonitorRect| {
        cursor.0 >= x as f64 && cursor.0 < x as f64 + w as f64 && cursor.1 >= y as f64 && cursor.1 < y as f64 + h as f64
    };
    let &(x, y, w, h, s) = monitors.iter().find(inside).or(monitors.first())?;
    Some((x as f64 + (w as f64 - OVERLAY_W * s) / 2.0, y as f64 + h as f64 - (OVERLAY_H + OVERLAY_BOTTOM_GAP) * s))
}

/// Move the overlay to the screen the user is working on. Tauri's
/// monitor_from_point expects logical points on macOS while cursor_position is
/// physical, so the monitor is matched here instead.
fn place_overlay(app: &AppHandle) {
    let Some(w) = app.get_webview_window("overlay") else { return };
    let Ok(monitors) = app.available_monitors() else { return };
    let rects: Vec<MonitorRect> = monitors
        .iter()
        .map(|m| (m.position().x, m.position().y, m.size().width, m.size().height, m.scale_factor()))
        .collect();
    let cursor = app.cursor_position().map(|p| (p.x, p.y)).unwrap_or((f64::MIN, f64::MIN));
    if let Some((x, y)) = overlay_position(&rects, cursor) {
        let _ = w.set_size(LogicalSize::new(OVERLAY_W, OVERLAY_H));
        let _ = w.set_position(PhysicalPosition::new(x, y));
    }
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(data_dir.join("models"))?;
    let first_run = !data_dir.join("settings.json").exists();
    let settings: Settings = store::load_or_set_aside(&data_dir.join("settings.json"));
    let settings = Arc::new(Mutex::new(settings));
    let asr = Arc::new(LazyAsr::new(data_dir.join("models"), settings.clone()));
    let deps = Arc::new(Deps {
        platform: platform::current(),
        asr: asr.clone(),
        provider: Arc::new(SwitchProvider { settings: settings.clone() }),
        dict: Arc::new(Mutex::new(store::load_or_set_aside(&data_dir.join("dictionary.json")))),
        history: Arc::new(Mutex::new(store::load_or_set_aside(&data_dir.join("history.json")))),
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
    let hotkey = hotkey::spawn(key, raw_tx.clone())
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
    app.manage(asr.clone());
    std::thread::spawn(move || asr.preload());

    if let Some(o) = app.get_webview_window("overlay") {
        let _ = o.set_ignore_cursor_events(true);
        place_overlay(app.handle());
        let _ = o.show();
    }

    // Standard macOS app menu plus Settings… (Cmd+,), which opens the main window.
    #[cfg(target_os = "macos")]
    {
        let menu = Menu::default(app.handle())?;
        if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.first() {
            let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
            app_menu.insert_items(&[&settings, &PredefinedMenuItem::separator(app)?], 2)?;
        }
        app.set_menu(menu)?;
        app.on_menu_event(|app, e| {
            if e.id() == "settings" {
                show_main(app);
            }
        });
    }

    let open = MenuItem::with_id(app, "open", "Open Shabdly", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Shabdly", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let tray = TrayIconBuilder::new().menu(&menu).tooltip("Shabdly").on_menu_event(|app, e| match e.id.as_ref() {
        "open" => show_main(app),
        "quit" => app.exit(0),
        _ => {}
    });
    // macOS tints a template icon to match a light or dark menu bar.
    #[cfg(target_os = "macos")]
    let tray = tray.icon(tauri::include_image!("icons/tray.png")).icon_as_template(true);
    #[cfg(not(target_os = "macos"))]
    let tray = match app.default_window_icon() {
        Some(icon) => tray.icon(icon.clone()),
        None => tray,
    };
    tray.build(app)?;

    app.manage(AppState { deps, hotkey: Mutex::new(hotkey), warm, last_hotkey_ms, hotkey_tx: raw_tx });
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
            commands::needs_setup,
            commands::globe_key_warning,
            commands::ollama_status,
            commands::pull_model,
            commands::whisper_ready,
            commands::download_whisper,
            commands::request_accessibility,
            commands::accessibility_status,
            commands::request_microphone,
            commands::open_settings_pane,
            commands::hotkey_health,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Shabdly");
    app.run(|app, event| {
        // Clicking the Dock icon brings the settings window back.
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen { .. } = event {
            show_main(app);
        }
        if let RunEvent::Exit = event {
            // Free the local model's memory; never block quitting for long.
            tauri::async_runtime::block_on(async {
                let _ = tokio::time::timeout(Duration::from_secs(1), unload(&OllamaProvider::default())).await;
            });
            if let Some(asr) = app.try_state::<Arc<LazyAsr>>() {
                asr.unload();
            }
        }
    });
}
