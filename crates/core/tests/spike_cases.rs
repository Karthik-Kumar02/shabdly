//! Live acceptance test (spec acceptance 4): needs Ollama with gemma4:e4b.
//! Run: cargo test -p shabdly-core --test spike_cases -- --ignored
mod common;
use serde::Deserialize;
use shabdly_core::pipeline::clean;
use shabdly_core::providers::OllamaProvider;
use shabdly_core::settings::Language;

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

#[tokio::test]
#[ignore]
async fn spike_cases_pass_through_pipeline() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("fixtures/spike_cases.json")).unwrap();
    let dict = common::dict();
    let provider = OllamaProvider::default();
    let mut failures = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let out = clean(&c.raw, &dict, Language::Auto, &provider).await;
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
        bad.extend(c.must_contain.iter().filter(|s| !out.text.contains(s.as_str())).map(|s| format!("missing {s}")));
        bad.extend(c.must_contain_ci.iter().filter(|s| !low.contains(&s.to_lowercase())).map(|s| format!("missing ci {s}")));
        bad.extend(c.must_not_words.iter().filter(|s| words.contains(&s.to_lowercase())).map(|s| format!("has word {s}")));
        println!("case {}: {} {}", i + 1, if bad.is_empty() { "ok  " } else { "FAIL" }, out.text);
        if !bad.is_empty() {
            failures.push(format!("case {}: {:?} -> {}", i + 1, bad, out.text));
        }
    }
    assert!(failures.is_empty(), "{} of {} failed:\n{}", failures.len(), cases.len(), failures.join("\n"));
}
