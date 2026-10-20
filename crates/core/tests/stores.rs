use shabdly_core::dictionary::Dictionary;
use shabdly_core::history::{History, HistoryItem};
use shabdly_core::settings::{Language, ProviderKind, Settings};
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
