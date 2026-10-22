use shabdly_core::dictionary::Dictionary;
use shabdly_core::history::{History, HistoryItem};
use shabdly_core::settings::{Language, LocalEngineKind, ProviderKind, Settings};
use shabdly_core::store;

fn item(at_ms: i64) -> HistoryItem {
    HistoryItem { at_ms, raw: "r".into(), cleaned: "c".into(), final_text: None, app: None }
}

#[test]
fn upsert_then_get_case_insensitive() {
    let mut d = Dictionary::default();
    d.upsert_manual("Karthik", vec!["Kartik".into()], None, 10);
    let e = d.get("karthik").expect("found");
    assert_eq!(e.word, "Karthik");
    assert_eq!(e.heard_as, vec!["Kartik"]);
    assert_eq!(e.created, 10);
}

#[test]
fn remove_and_star() {
    let mut d = Dictionary::default();
    d.upsert_manual("Tauri", vec![], None, 1);
    assert!(d.set_starred("tauri", true));
    assert!(d.get("Tauri").unwrap().starred);
    assert!(!d.set_starred("nope", true));
    assert!(d.remove("TAURI"));
    assert!(d.get("Tauri").is_none());
    assert!(!d.remove("Tauri"));
}

#[test]
fn hints_order_starred_then_uses() {
    let mut d = Dictionary::default();
    d.upsert_manual("Alpha", vec!["alfa".into()], None, 1);
    d.upsert_manual("Beta", vec!["bayta".into()], None, 1);
    d.upsert_manual("Gamma", vec![], None, 1);
    for _ in 0..5 { d.record_use("Beta", 2); }
    d.record_use("Gamma", 2);
    d.set_starred("Alpha", true);
    let h = d.hints(10);
    let words: Vec<_> = h.iter().map(|(w, _)| w.as_str()).collect();
    assert_eq!(words, vec!["Alpha", "Beta", "Gamma"]);
    assert_eq!(h[0].1, vec!["alfa"]);
    assert_eq!(d.hints(2).len(), 2);
}

#[test]
fn hints_skip_replace_with() {
    let mut d = Dictionary::default();
    d.upsert_manual("addr", vec![], Some("12 Park Street".into()), 1);
    d.upsert_manual("REA", vec!["ria".into()], None, 1);
    d.get_mut("REA").unwrap().hint_only.push("reya".into());
    let h = d.hints(10);
    assert_eq!(h, vec![("REA".to_string(), vec!["ria".to_string(), "reya".to_string()])]);
}

#[test]
fn import_merges_by_word() {
    let mut d = Dictionary::default();
    d.upsert_manual("Karthik", vec!["Kartik".into()], None, 1);
    let mut other = Dictionary::default();
    other.upsert_manual("karthik", vec!["Kartik".into(), "Kartic".into()], None, 2);
    other.upsert_manual("Tauri", vec!["tory".into()], None, 2);
    let added = d.import_json(&other.export_json()).unwrap();
    assert_eq!(added, 1);
    assert_eq!(d.get("Karthik").unwrap().heard_as, vec!["Kartik", "Kartic"]);
    assert!(d.get("Tauri").is_some());
    assert_eq!(d.entries.len(), 2);
}

#[test]
fn history_caps_at_100() {
    let mut h = History::default();
    for i in 0..105 { h.push(item(i)); }
    assert_eq!(h.items.len(), 100);
    assert_eq!(h.items.front().unwrap().at_ms, 5);
    h.set_final(104, "done".into());
    assert_eq!(h.items.back().unwrap().final_text.as_deref(), Some("done"));
    h.clear();
    assert!(h.items.is_empty());
}

#[test]
fn settings_defaults() {
    let s = Settings::default();
    assert_eq!(s.version, 1);
    assert_eq!(s.language, Language::Auto);
    assert_eq!(s.provider, ProviderKind::Local);
    assert!(s.learn_from_corrections);
    assert!(s.keep_history);
    assert_eq!(s.whisper_model, "ggml-large-v3-turbo-q8_0.bin");
    assert!(!s.hotkey.is_empty());
}

#[test]
fn save_load_roundtrip_and_missing_file_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dictionary.json");
    let empty: Dictionary = store::load(&path).unwrap();
    assert_eq!(empty.version, 1);
    assert!(empty.entries.is_empty());
    let mut d = Dictionary::default();
    d.upsert_manual("Karthik", vec![], None, 1);
    store::save(&path, &d).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.replace([' ', '\n'], "").contains("\"version\":1"));
    let back: Dictionary = store::load(&path).unwrap();
    assert_eq!(back.entries.len(), 1);
    assert!(!path.with_extension("json.tmp").exists());
}

#[test]
fn corrupt_file_is_set_aside_not_overwritten() {
    use shabdly_core::store::load_or_set_aside;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dictionary.json");
    std::fs::write(&path, "{ not json").unwrap();
    let d: shabdly_core::dictionary::Dictionary = load_or_set_aside(&path);
    assert!(d.entries.is_empty());
    assert!(!path.exists(), "bad file moved out of the way");
    let kept: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert!(kept.iter().any(|n| n.starts_with("dictionary.json.bad-")), "{kept:?}");
    assert_eq!(std::fs::read_to_string(dir.path().join(&kept[0])).unwrap(), "{ not json");
    // A missing file is just the default, nothing set aside.
    let _: shabdly_core::dictionary::Dictionary = load_or_set_aside(&dir.path().join("history.json"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn v1_file_loads_as_ollama_upgrader() {
    let v1 = r#"{"version":1,"language":"auto","provider":"local","hotkey":"Fn","learn_from_corrections":true,"keep_history":true,"whisper_model":"ggml-large-v3-turbo-q8_0.bin"}"#;
    let s: Settings = serde_json::from_str(v1).unwrap();
    assert_eq!(s.local_engine, LocalEngineKind::Ollama);
    assert_eq!(s.speed, None);
    assert!(!s.builtin_offer_done);
}

#[test]
fn new_install_defaults_to_bundled() {
    let s = Settings::default();
    assert_eq!(s.local_engine, LocalEngineKind::Bundled);
    assert!(s.builtin_offer_done);
}

#[test]
fn v2_file_still_parses_with_015_struct() {
    // The 0.1.5 types, copied as they shipped: a v2 file must not trip them.
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    #[allow(dead_code)]
    enum Language015 { English, Hindi, Hinglish, Auto }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    #[allow(dead_code)]
    enum ProviderKind015 { Local, GeminiFree, GeminiPaid, Haiku }
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct Settings015 {
        version: u32,
        language: Language015,
        provider: ProviderKind015,
        hotkey: String,
        learn_from_corrections: bool,
        keep_history: bool,
        whisper_model: String,
    }
    let v2 = serde_json::to_string(&Settings::default()).unwrap();
    serde_json::from_str::<Settings015>(&v2).unwrap();
}

#[test]
fn backup_v1_once_copies_only_once() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("settings.json"), "a").unwrap();
    store::backup_v1_once(dir.path());
    std::fs::write(dir.path().join("settings.json"), "b").unwrap();
    store::backup_v1_once(dir.path());
    assert_eq!(std::fs::read_to_string(dir.path().join("settings.v1.json")).unwrap(), "a");
}
