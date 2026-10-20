use shabdly_core::dictionary::{Dictionary, Source};
use shabdly_core::learning::{LearnEvent, diff_candidates, is_common, learn, undo};
use shabdly_core::swaps::apply_swaps;

#[test]
fn learns_new_word() {
    let mut d = Dictionary::default();
    let ev = learn(&mut d, "ask Kartik now", "ask Karthik now", 1);
    assert_eq!(ev, vec![LearnEvent::Added { word: "Karthik".into(), heard: "Kartik".into() }]);
    assert_eq!(d.get("Karthik").unwrap().heard_as, vec!["Kartik"]);
    assert_eq!(d.get("Karthik").unwrap().source, Source::Learned);
    assert_eq!(d.get("Karthik").unwrap().created, 1);
}

#[test]
fn existing_word_gains_heard_as() {
    let mut d = Dictionary::default();
    d.upsert_manual("Karthik", vec![], None, 1);
    let ev = learn(&mut d, "ask Kartik now", "ask Karthik now", 2);
    assert_eq!(ev, vec![LearnEvent::HeardAdded { word: "Karthik".into(), heard: "Kartik".into() }]);
    assert_eq!(d.get("Karthik").unwrap().heard_as, vec!["Kartik"]);
    assert_eq!(d.entries.len(), 1);
    assert_eq!(d.get("Karthik").unwrap().source, Source::Manual);
    assert!(learn(&mut d, "ask Kartik now", "ask Karthik now", 3).is_empty());
}

#[test]
fn skips_more_than_four_words() {
    let mut d = Dictionary::default();
    assert!(learn(&mut d, "a b c d e f", "v w x y z f", 1).is_empty());
    assert!(d.entries.is_empty());
}

#[test]
fn skips_common_corrected_side() {
    let mut d = Dictionary::default();
    assert!(learn(&mut d, "go then", "go the", 1).is_empty());
}

#[test]
fn skips_insert_and_delete_only() {
    let mut d = Dictionary::default();
    assert!(learn(&mut d, "hello", "hello there", 1).is_empty());
    assert!(learn(&mut d, "hello there", "hello", 1).is_empty());
}

#[test]
fn real_word_needs_two_corrections() {
    let mut d = Dictionary::default();
    assert!(is_common("their"));
    // "their" is common: first correction is hint-only, second promotes to a swap
    assert!(matches!(learn(&mut d, "their car", "there car", 1)[0], LearnEvent::HintOnly { .. }));
    assert_eq!(apply_swaps("their car", &d).0, "their car");
    assert_eq!(d.hints(10), vec![("there".to_string(), vec!["their".to_string()])]);
    assert!(matches!(learn(&mut d, "their bus", "there bus", 2)[0], LearnEvent::Promoted { .. }));
    assert_eq!(apply_swaps("take their car", &d).0, "take there car");
}

#[test]
fn case_only_change_is_learned() {
    let mut d = Dictionary::default();
    let ev = learn(&mut d, "push to rea", "push to REA", 1);
    assert_eq!(ev, vec![LearnEvent::Added { word: "REA".into(), heard: "rea".into() }]);
}

#[test]
fn ignores_trailing_punctuation_noise() {
    let mut d = Dictionary::default();
    assert!(learn(&mut d, "hello world", "hello world.", 1).is_empty());
    assert!(diff_candidates("hello world", "hello, world!").is_empty());
}

#[test]
fn case_only_change_to_common_word_is_skipped() {
    let mut d = Dictionary::default();
    assert!(learn(&mut d, "so i think hello", "So I think Hello", 1).is_empty());
}

#[test]
fn undo_reverts_added_and_heard_added() {
    let mut d = Dictionary::default();
    let ev = learn(&mut d, "ask Kartik now", "ask Karthik now", 1);
    undo(&mut d, &ev[0]);
    assert!(d.get("Karthik").is_none());

    d.upsert_manual("Tauri", vec!["tory".into()], None, 1);
    let ev = learn(&mut d, "the tauree branch", "the Tauri branch", 2);
    assert_eq!(ev, vec![LearnEvent::HeardAdded { word: "Tauri".into(), heard: "tauree".into() }]);
    undo(&mut d, &ev[0]);
    assert_eq!(d.get("Tauri").unwrap().heard_as, vec!["tory"]);
}
