//! Learning from corrections (spec s4): diff what was inserted against what the
//! user ended with, and turn each changed span into dictionary knowledge.
use crate::dictionary::{Dictionary, Entry, Source, add_form};
use crate::swaps::is_boundary;
use serde::{Deserialize, Serialize};
use similar::{Algorithm, DiffOp, capture_diff_slices};
use std::collections::HashSet;
use std::sync::OnceLock;

// ponytail: fixed list; swap for a frequency list if real-word swaps slip through.
const COMMON_WORDS: &str = include_str!("common_words.txt");

/// Function words: a correction whose new side is only these is noise, not a
/// word to learn ("then" to "the").
const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "to", "of", "and", "or", "in", "on", "at", "it", "i", "hai", "ka", "ki",
    "ke", "ko", "se", "aur", "to", "na", "bhi",
];

const MAX_WORDS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub original: String,
    pub corrected: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LearnEvent {
    Added { word: String, heard: String },
    HeardAdded { word: String, heard: String },
    HintOnly { word: String, heard: String },
    Promoted { word: String, heard: String },
}

fn common() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| COMMON_WORDS.lines().map(str::trim).filter(|l| !l.is_empty()).collect())
}

/// True when every word of the phrase is a common English or Hindi word.
pub fn is_common(phrase: &str) -> bool {
    let mut words = phrase.split_whitespace().peekable();
    words.peek().is_some() && words.all(|w| common().contains(w.to_lowercase().as_str()))
}

fn only_stop_words(phrase: &str) -> bool {
    phrase.split_whitespace().all(|w| STOP_WORDS.contains(&w.to_lowercase().as_str()))
}

fn tokens(text: &str) -> Vec<&str> {
    text.split_whitespace().map(|t| t.trim_matches(is_boundary)).filter(|t| !t.is_empty()).collect()
}

/// Each replaced span of words becomes one candidate. Pure inserts and deletes
/// carry no "heard as" information, so they are dropped.
pub fn diff_candidates(expected: &str, final_text: &str) -> Vec<Candidate> {
    let (a, b) = (tokens(expected), tokens(final_text));
    capture_diff_slices(Algorithm::Myers, &a, &b)
        .into_iter()
        .filter_map(|op| match op {
            DiffOp::Replace { old_index, old_len, new_index, new_len } => Some(Candidate {
                original: a[old_index..old_index + old_len].join(" "),
                corrected: b[new_index..new_index + new_len].join(" "),
            }),
            _ => None,
        })
        .collect()
}

fn has_form(list: &[String], form: &str) -> bool {
    let f = form.to_lowercase();
    list.iter().any(|x| x.to_lowercase() == f)
}

fn remove_form(list: &mut Vec<String>, form: &str) {
    let f = form.to_lowercase();
    list.retain(|x| x.to_lowercase() != f);
}

pub fn learn(dict: &mut Dictionary, expected: &str, final_text: &str, now: i64) -> Vec<LearnEvent> {
    let mut events = Vec::new();
    for Candidate { original, corrected } in diff_candidates(expected, final_text) {
        let (on, cn) = (original.split_whitespace().count(), corrected.split_whitespace().count());
        if on == 0 || cn == 0 || on > MAX_WORDS || cn > MAX_WORDS || only_stop_words(&corrected) {
            continue;
        }
        let real_word = is_common(&original);
        // Recapitalizing common words ("so" to "So") is sentence casing, not a word to learn.
        if real_word && original.to_lowercase() == corrected.to_lowercase() {
            continue;
        }
        let (word, heard) = (corrected.clone(), original.clone());
        match dict.get_mut(&corrected) {
            Some(e) if has_form(&e.hint_only, &original) => {
                remove_form(&mut e.hint_only, &original);
                add_form(&mut e.heard_as, &original);
                events.push(LearnEvent::Promoted { word: e.word.clone(), heard });
            }
            Some(e) if has_form(&e.heard_as, &original) => {}
            Some(e) if real_word => {
                add_form(&mut e.hint_only, &original);
                events.push(LearnEvent::HintOnly { word: e.word.clone(), heard });
            }
            Some(e) => {
                add_form(&mut e.heard_as, &original);
                events.push(LearnEvent::HeardAdded { word: e.word.clone(), heard });
            }
            None => {
                let mut e = Entry::new(&corrected, Source::Learned, now);
                if real_word {
                    e.hint_only.push(original);
                    events.push(LearnEvent::HintOnly { word, heard });
                } else {
                    e.heard_as.push(original);
                    events.push(LearnEvent::Added { word, heard });
                }
                dict.entries.push(e);
            }
        }
    }
    events
}

/// Reverse one event. An entry that a hint-only event created and that is now
/// empty is removed with it.
pub fn undo(dict: &mut Dictionary, event: &LearnEvent) {
    match event {
        LearnEvent::Added { word, .. } => {
            dict.remove(word);
        }
        LearnEvent::HeardAdded { word, heard } => {
            if let Some(e) = dict.get_mut(word) {
                remove_form(&mut e.heard_as, heard);
            }
        }
        LearnEvent::HintOnly { word, heard } => {
            let empty = match dict.get_mut(word) {
                Some(e) => {
                    remove_form(&mut e.hint_only, heard);
                    e.source == Source::Learned && e.heard_as.is_empty() && e.hint_only.is_empty()
                }
                None => false,
            };
            if empty {
                dict.remove(word);
            }
        }
        LearnEvent::Promoted { word, heard } => {
            if let Some(e) = dict.get_mut(word) {
                remove_form(&mut e.heard_as, heard);
                add_form(&mut e.hint_only, heard);
            }
        }
    }
}
