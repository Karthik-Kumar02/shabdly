mod common;
use async_trait::async_trait;
use common::dict;
use serde_json::json;
use shabdly_core::dictionary::Dictionary;
use shabdly_core::pipeline::{clean, timeout_for};
use shabdly_core::prompt::system_prompt;
use shabdly_core::providers::{CleanupProvider, parse_anthropic, parse_gemini, parse_ollama};
use shabdly_core::settings::Language;
use std::time::Duration;

enum Fake {
    Echo(&'static str),
    Fail,
    Slow,
    /// Transliterates like the real model does, losing the Devanagari form.
    Translit,
}

#[async_trait]
impl CleanupProvider for Fake {
    fn base_timeout(&self) -> Duration {
        Duration::from_millis(50)
    }
    async fn clean(&self, _system: &str, _text: &str) -> anyhow::Result<String> {
        match self {
            Fake::Echo(s) => Ok(s.to_string()),
            Fake::Translit => Ok(_text.replace("रिया", "Rhea")),
            Fake::Fail => anyhow::bail!("connection refused"),
            Fake::Slow => {
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok("too late".into())
            }
        }
    }
}

#[test]
fn prompt_hinglish_rule_and_hints() {
    let p = system_prompt(Language::Hinglish, &dict().hints(200));
    assert!(p.contains("never Devanagari"));
    assert!(p.contains("Karthik (heard as Kartik/कार्तिक)"));
    assert!(p.contains("Output only the cleaned text."));
}

#[test]
fn prompt_omits_dictionary_line_without_hints() {
    let p = system_prompt(Language::Auto, &[]);
    assert!(!p.contains("dictionary words"));
    assert!(p.contains("never Devanagari"));
}

#[test]
fn prompt_hindi_rule() {
    let p = system_prompt(Language::Hindi, &[]);
    assert!(p.contains("Write Hindi in Devanagari script."));
    assert!(!p.contains("never Devanagari"));
}

#[test]
fn parse_ollama_gemini_anthropic_ok() {
    assert_eq!(parse_ollama(&json!({"message": {"role": "assistant", "content": " Hi. "}})).unwrap(), "Hi.");
    let g = json!({"candidates": [{"content": {"parts": [{"text": "Hello "}, {"text": "there."}]}}]});
    assert_eq!(parse_gemini(&g).unwrap(), "Hello there.");
    let a = json!({"content": [{"type": "text", "text": "Done."}], "stop_reason": "end_turn"});
    assert_eq!(parse_anthropic(&a).unwrap(), "Done.");
}

#[test]
fn parse_anthropic_refusal_is_error() {
    let a = json!({"content": [], "stop_reason": "refusal"});
    assert!(parse_anthropic(&a).is_err());
}

#[tokio::test]
async fn pipeline_applies_swaps_after_provider() {
    let c = clean("ask kartik", &dict(), Language::Auto, &Fake::Echo("ask Kartik")).await;
    assert_eq!(c.text, "ask Karthik");
    assert!(!c.cleanup_skipped);
    assert_eq!(c.used_words, vec!["Karthik"]);
}

#[tokio::test]
async fn pipeline_falls_back_on_error() {
    let c = clean(" ask kartik ", &dict(), Language::Auto, &Fake::Fail).await;
    assert_eq!(c.text, "ask Karthik");
    assert!(c.cleanup_skipped);
}

#[tokio::test]
async fn pipeline_falls_back_on_timeout() {
    let c = clean("tory branch", &dict(), Language::Auto, &Fake::Slow).await;
    assert_eq!(c.text, "Tauri branch");
    assert!(c.cleanup_skipped);
}

#[tokio::test]
async fn pipeline_falls_back_on_empty_output() {
    let c = clean("hello", &Dictionary::default(), Language::Auto, &Fake::Echo("  ")).await;
    assert_eq!(c.text, "hello");
    assert!(c.cleanup_skipped);
}

#[test]
fn timeout_scales_with_length() {
    let s = Duration::from_secs(8);
    assert_eq!(timeout_for(s, &"word ".repeat(151)), Duration::from_secs(16));
    assert_eq!(timeout_for(s, &"word ".repeat(10)), s);
    assert_eq!(timeout_for(s, &"word ".repeat(150)), s);
}

#[tokio::test]
async fn pipeline_swaps_before_provider_too() {
    let c = clean("रिया वाला setup", &dict(), Language::Auto, &Fake::Translit).await;
    assert_eq!(c.text, "REA वाला setup");
    assert!(!c.cleanup_skipped);
}
