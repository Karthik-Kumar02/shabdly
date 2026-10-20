//! Cleanup pipeline: layer 1 hints to the model, then layer 2 exact swaps.
//! Any provider failure falls back to the raw transcript (spec s6).
use crate::dictionary::Dictionary;
use crate::prompt::system_prompt;
use crate::providers::{CleanupProvider, OllamaProvider};
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
    pub cleanup_skipped: bool,
    pub used_words: Vec<String>,
}

pub async fn clean(raw: &str, dict: &Dictionary, lang: Language, provider: &dyn CleanupProvider) -> Cleaned {
    // Fix heard-as forms before the model too: it transliterates Devanagari
    // forms ("रिया" to "Rhea") so the post-cleanup pass alone would never see
    // them. Snippets expand only afterward, so they never reach a provider.
    let (pre, mut used_words) = apply_heard_swaps(raw.trim(), dict);
    let system = system_prompt(lang, &dict.hints(200));
    let limit = timeout_for(provider.base_timeout(), &pre);
    let model = match tokio::time::timeout(limit, provider.clean(&system, &pre)).await {
        Ok(Ok(s)) if !s.trim().is_empty() => Some(s),
        _ => None,
    };
    let cleanup_skipped = model.is_none();
    let (text, post_used) = apply_swaps(model.as_deref().unwrap_or(&pre), dict);
    for w in post_used {
        if !used_words.contains(&w) {
            used_words.push(w);
        }
    }
    Cleaned { text: text.trim().to_string(), cleanup_skipped, used_words }
}

/// Load the local model so the first dictation is fast.
pub async fn warm_up(p: &OllamaProvider) -> anyhow::Result<()> {
    p.chat("Reply with OK.", "OK", -1).await.map(|_| ())
}

/// Release the local model's memory when the app quits.
pub async fn unload(p: &OllamaProvider) -> anyhow::Result<()> {
    p.chat("", "", 0).await.map(|_| ())
}
