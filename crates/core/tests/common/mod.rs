#![allow(dead_code)]
use shabdly_core::dictionary::Dictionary;

/// The spike's dictionary, shared by the swap, pipeline, and spike tests.
pub fn dict() -> Dictionary {
    let mut d = Dictionary::default();
    let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    d.upsert_manual("Karthik", v(&["Kartik", "कार्तिक"]), None, 1);
    d.upsert_manual("REA", v(&["ria", "reya", "रिया"]), None, 1);
    d.upsert_manual("Tauri", v(&["tory"]), None, 1);
    d.upsert_manual("Sanjeev", v(&["संजीव"]), None, 1);
    d.upsert_manual("Shabdly", v(&["shabd lee", "shabdlee"]), None, 1);
    d.upsert_manual("ghee", v(&["gee"]), None, 1);
    d.upsert_manual("addr", vec![], Some("12 Park Street".into()), 1);
    d
}
