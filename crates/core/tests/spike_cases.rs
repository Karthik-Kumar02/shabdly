//! Live release gate (spec section 6): 15/15 cases pass and at least 14 outputs end in `.`, `?` or `!`.
//! Default backend is Ollama with gemma4:e4b. Others come from env:
//! BENCH_BACKEND=ollama|openai, BENCH_URL, BENCH_MODEL, BENCH_TOKEN (openai only).
//! BENCH_PUNCT_FIX=1 applies `finish_sentence_from`, as `LocalEngine` does for the fast (E2B) models.
//! Run: cargo test --release -p shabdly-core --test spike_cases -- --ignored --nocapture
mod common;
use serde::Deserialize;
use async_trait::async_trait;
use shabdly_core::pipeline::{clean, finish_sentence_from};
use shabdly_core::providers::{CleanupProvider, OllamaProvider, OpenAiProvider};
use shabdly_core::settings::Language;
use std::time::{Duration, Instant};

#[derive(Deserialize)]
struct Case {
    raw: String,
    #[serde(default)]
    must_contain: Vec<String>,
    #[serde(default)]
    must_contain_ci: Vec<String>,
    #[serde(default)]
    must_not_words: Vec<String>,
}

struct Punctuated(Box<dyn CleanupProvider>);

#[async_trait]
impl CleanupProvider for Punctuated {
    fn base_timeout(&self) -> Duration {
        self.0.base_timeout()
    }
    async fn clean(&self, system: &str, text: &str) -> anyhow::Result<String> {
        self.0.clean(system, text).await.map(|t| finish_sentence_from(&t, text))
    }
}

fn provider() -> Box<dyn CleanupProvider> {
    let p = backend();
    if std::env::var("BENCH_PUNCT_FIX").is_ok_and(|v| v == "1") { Box::new(Punctuated(p)) } else { p }
}

fn backend() -> Box<dyn CleanupProvider> {
    let env = |k: &str| std::env::var(k).ok();
    match env("BENCH_BACKEND").as_deref() {
        Some("openai") => Box::new(OpenAiProvider {
            base_url: env("BENCH_URL").expect("BENCH_URL"),
            token: env("BENCH_TOKEN").unwrap_or_default(),
            model: env("BENCH_MODEL").expect("BENCH_MODEL"),
        }),
        None | Some("ollama") => {
            let d = OllamaProvider::default();
            Box::new(OllamaProvider {
                base_url: env("BENCH_URL").unwrap_or(d.base_url),
                model: env("BENCH_MODEL").unwrap_or(d.model),
            })
        }
        Some(b) => panic!("unknown BENCH_BACKEND {b}"),
    }
}

#[tokio::test]
#[ignore]
async fn spike_cases_pass_through_pipeline() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/spike_cases.json")).unwrap();
    let dict = common::dict();
    let provider = provider();
    // Warm-up so the first case's time excludes model loading.
    provider.clean("Fix punctuation.", "hello").await.expect("warm-up call failed");
    let (mut failures, mut punct, mut ms) = (Vec::new(), 0, Vec::new());
    for (i, c) in cases.iter().enumerate() {
        let t = Instant::now();
        let out = clean(&c.raw, &dict, Language::Auto, provider.as_ref()).await;
        ms.push(t.elapsed().as_millis());
        let low = out.text.to_lowercase();
        let words: Vec<String> =
            low.split(|ch: char| !ch.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect();
        let mut bad = Vec::new();
        if out.cleanup_skipped {
            bad.push("cleanup skipped".to_string());
        }
        if out.text.chars().any(|ch| ('\u{0900}'..='\u{097F}').contains(&ch)) {
            bad.push("has Devanagari".into());
        }
        // A model that answers the question or rambles produces far more text than it was given.
        if out.text.chars().count() > 2 * c.raw.chars().count() + 10 {
            bad.push("too long".into());
        }
        bad.extend(c.must_contain.iter().filter(|s| !out.text.contains(s.as_str())).map(|s| format!("missing {s}")));
        bad.extend(c.must_contain_ci.iter().filter(|s| !low.contains(&s.to_lowercase())).map(|s| format!("missing ci {s}")));
        bad.extend(c.must_not_words.iter().filter(|s| words.contains(&s.to_lowercase())).map(|s| format!("has word {s}")));
        if out.text.ends_with(['.', '?', '!']) {
            punct += 1;
        }
        println!("case {}: {} {:>5} ms {}", i + 1, if bad.is_empty() { "ok  " } else { "FAIL" }, ms[i], out.text);
        if !bad.is_empty() {
            failures.push(format!("case {}: {:?} -> {}", i + 1, bad, out.text));
        }
    }
    let n = cases.len();
    ms.sort();
    println!("SUMMARY pass {}/{n} punct {punct}/{n} median {} ms max {} ms", n - failures.len(), ms[n / 2], ms[n - 1]);
    assert!(failures.is_empty(), "{} of {n} failed:\n{}", failures.len(), failures.join("\n"));
    assert!(punct >= n - 1, "only {punct} of {n} outputs end in . ? or !");
}
