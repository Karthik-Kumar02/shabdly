//! Cleanup pipeline: layer 1 hints to the model, then layer 2 exact swaps.
//! Any provider failure falls back to the raw transcript (spec s6).
use crate::dictionary::Dictionary;
use crate::prompt::system_prompt;
use crate::providers::{CleanupProvider, OllamaProvider, Skip};
use crate::settings::Language;
use crate::swaps::{apply_heard_swaps, apply_swaps};
use std::time::Duration;

const WORDS_PER_STEP: usize = 150;

/// Long dictations get proportionally more time: one base timeout per 150 words.
pub fn timeout_for(base: Duration, text: &str) -> Duration {
    let words = text.split_whitespace().count();
    base * words.div_ceil(WORDS_PER_STEP).max(1) as u32
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cleaned {
    pub text: String,
    /// Why the model's output was not used; None when it was.
    pub skipped: Option<Skip>,
    pub used_words: Vec<String>,
}

pub async fn clean(raw: &str, dict: &Dictionary, lang: Language, provider: &dyn CleanupProvider) -> Cleaned {
    // Fix heard-as forms before the model too: it transliterates Devanagari
    // forms ("रिया" to "Rhea") so the post-cleanup pass alone would never see
    // them. Snippets expand only afterward, so they never reach a provider.
    let (pre, mut used_words) = apply_heard_swaps(raw.trim(), dict);
    let system = system_prompt(lang, &dict.hints(200));
    let limit = timeout_for(provider.base_timeout(), &pre);
    let (model, skipped) = match tokio::time::timeout(limit, provider.clean(&system, &pre)).await {
        Ok(Ok(s)) if !s.trim().is_empty() => (Some(s), None),
        Ok(Ok(_)) => (None, Some(Skip::EngineError)),
        Ok(Err(e)) => (None, Some(e.downcast_ref::<Skip>().copied().unwrap_or(Skip::EngineError))),
        Err(_) => (None, Some(Skip::TooSlow)),
    };
    let (text, post_used) = apply_swaps(model.as_deref().unwrap_or(&pre), dict);
    for w in post_used {
        if !used_words.contains(&w) {
            used_words.push(w);
        }
    }
    Cleaned { text: text.trim().to_string(), skipped, used_words }
}

/// Load the local model so the first dictation is fast.
pub async fn warm_up(p: &OllamaProvider) -> anyhow::Result<()> {
    p.chat("Reply with OK.", "OK", -1).await.map(|_| ())
}

/// Release the local model's memory when the app quits.
pub async fn unload(p: &OllamaProvider) -> anyhow::Result<()> {
    p.chat("", "", 0).await.map(|_| ())
}

const QUESTION_WORDS_HI: &[&str] = &[
    "kya", "kyun", "kyon", "kyu", "kaise", "kab", "kahan", "kahaan", "kaun", "kitna", "kitni", "kitne", "क्या", "क्यों",
    "कैसे", "कब", "कहाँ", "कहां", "कौन", "कितना",
];
const QUESTION_WORDS_EN: &[&str] = &["what", "why", "how", "when", "where", "who", "which", "whose"];
const AUXILIARIES: &[&str] =
    &["can", "could", "would", "will", "should", "shall", "may", "do", "does", "did", "is", "are", "was", "were", "have", "has"];
const SUBJECTS: &[&str] = &["i", "you", "we", "they", "he", "she", "it", "this", "that", "there"];

fn is_end_mark(c: char) -> bool {
    matches!(c, '.' | '?' | '!' | '।')
}

/// Upper-cases the first letter unless the word is lower camel case ("iPhone").
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(first), second) if first.is_lowercase() && !second.is_some_and(char::is_uppercase) => {
            first.to_uppercase().chain(s[first.len_utf8()..].chars()).collect()
        }
        _ => s.to_string(),
    }
}

/// The E2B models often drop the final punctuation (spec risk Q1). Ends the last sentence with
/// `?` for clear question forms, else `.` (`।` after Devanagari), and capitalizes its first letter.
/// Text already ending in `.`, `?`, `!` or `।` (closing quotes and brackets aside) is returned as is.
pub fn finish_sentence(text: &str) -> String {
    let t = text.trim_end();
    let Some(last) = t.trim_end_matches(['"', '\'', '”', '’', ')', ']']).chars().last() else { return text.to_string() };
    if is_end_mark(last) {
        return text.to_string();
    }
    let start = t
        .char_indices()
        .filter(|&(i, c)| is_end_mark(c) && t[i + c.len_utf8()..].starts_with(char::is_whitespace))
        .map(|(i, c)| i + c.len_utf8())
        .last()
        .unwrap_or(0);
    let (head, tail) = t.split_at(start);
    let sentence = tail.trim_start();
    let words: Vec<String> = sentence
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| c.is_ascii_punctuation()).to_lowercase())
        .collect();
    let has = |list: &[&str], w: Option<&String>| w.is_some_and(|w| list.contains(&w.as_str()));
    // ponytail: word lists, not a parser. Hinglish puts the question word anywhere ("biryani kaise
    // banate hain"), so "pata nahi kab aayega" also gets `?`; English needs it first and not
    // followed by a subject ("when you get a chance ..." is a statement).
    let question = words.iter().any(|w| has(QUESTION_WORDS_HI, Some(w)))
        || (has(QUESTION_WORDS_EN, words.first()) && !sentence.contains(',') && !has(SUBJECTS, words.get(1)))
        || (has(AUXILIARIES, words.first()) && has(SUBJECTS, words.get(1)));
    let mark = if question {
        '?'
    } else if ('\u{0900}'..='\u{097F}').contains(&last) {
        '।'
    } else {
        '.'
    };
    format!("{}{}{}{mark}", capitalize(head), &tail[..tail.len() - sentence.len()], capitalize(sentence))
}

/// `finish_sentence` for a cleanup of `raw`: when Whisper heard a question (`raw` ends in `?`)
/// and the model dropped the mark, the sentence ends in `?` whatever the word lists say.
pub fn finish_sentence_from(text: &str, raw: &str) -> String {
    let out = finish_sentence(text);
    let asked = raw.trim_end().trim_end_matches(['"', '\'', '”', '’', ')', ']']).ends_with('?');
    match out.strip_suffix(['.', '।']) {
        Some(head) if asked && out != text => format!("{head}?"),
        _ => out,
    }
}
