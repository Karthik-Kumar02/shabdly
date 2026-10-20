mod common;
use common::dict;
use shabdly_core::dictionary::Dictionary;
use shabdly_core::swaps::apply_swaps;

#[test]
fn swaps_whole_words_case_insensitive() {
    assert_eq!(apply_swaps("ask kartik about tory", &dict()).0, "ask Karthik about Tauri");
}

#[test]
fn swaps_multi_word_longest_first() {
    assert_eq!(apply_swaps("Shabd Lee ka feature", &dict()).0, "Shabdly ka feature");
}

#[test]
fn no_partial_word_match() {
    assert_eq!(apply_swaps("victory story", &dict()).0, "victory story");
}

#[test]
fn devanagari_forms_match_whole_word() {
    assert_eq!(apply_swaps("रिया वाला setup", &dict()).0, "REA वाला setup");
    assert_eq!(apply_swaps("रियासत", &dict()).0, "रियासत");
}

#[test]
fn sentence_start_capitalizes_lowercase_word() {
    assert_eq!(apply_swaps("gee is good. gee again", &dict()).0, "Ghee is good. Ghee again");
}

#[test]
fn punctuation_is_boundary() {
    assert_eq!(apply_swaps("kartik, reya!", &dict()).0, "Karthik, REA!");
}

#[test]
fn replace_with_expands() {
    assert_eq!(apply_swaps("send it to addr please", &dict()).0, "send it to 12 Park Street please");
}

#[test]
fn hint_only_forms_do_not_swap() {
    let mut d = Dictionary::default();
    d.upsert_manual("there", vec![], None, 1);
    d.get_mut("there").unwrap().hint_only.push("their".into());
    assert_eq!(apply_swaps("their car", &d).0, "their car");
}

#[test]
fn reports_used_words() {
    assert_eq!(apply_swaps("kartik and tory", &dict()).1, vec!["Karthik", "Tauri"]);
}
