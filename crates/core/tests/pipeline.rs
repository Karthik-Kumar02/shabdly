mod common;
use async_trait::async_trait;
use common::dict;
use serde_json::json;
use shabdly_core::dictionary::Dictionary;
use shabdly_core::pipeline::{clean, finish_sentence, timeout_for};
use shabdly_core::prompt::system_prompt;
use shabdly_core::providers::{
    CleanupProvider, ollama_body, openai_body, parse_anthropic, parse_gemini, parse_ollama, parse_openai,
};
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

/// Records what the provider was sent, and echoes it back.
struct Capture(std::sync::Mutex<String>);

#[async_trait]
impl CleanupProvider for Capture {
    fn base_timeout(&self) -> Duration {
        Duration::from_millis(50)
    }
    async fn clean(&self, system: &str, text: &str) -> anyhow::Result<String> {
        *self.0.lock().unwrap() = format!("{system}\n{text}");
        Ok(text.to_string())
    }
}

#[tokio::test]
async fn snippets_never_reach_the_provider() {
    let p = Capture(Default::default());
    let c = clean("send it to addr please", &dict(), Language::Auto, &p).await;
    let sent = p.0.lock().unwrap().clone();
    assert!(!sent.contains("12 Park Street"), "provider saw the snippet: {sent}");
    assert_eq!(c.text, "send it to 12 Park Street please");
}

#[test]
fn parse_anthropic_truncated_is_error() {
    let a = json!({"content": [{"type": "text", "text": "Half of the"}], "stop_reason": "max_tokens"});
    assert!(parse_anthropic(&a).is_err());
}

#[test]
fn ollama_request_sets_context_window() {
    let b = ollama_body("gemma4:e4b", "sys", "hi", -1);
    assert_eq!(b["options"]["num_ctx"], 8192);
    assert_eq!(b["options"]["temperature"], 0);
    assert_eq!(b["think"], false);
}

#[test]
fn openai_body_has_fixed_params() {
    let b = openai_body("gemma-4-e4b", "sys", "hi");
    assert_eq!(b["temperature"], 0);
    assert_eq!(b["max_tokens"], 4096);
    assert_eq!(b["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(b["messages"][0]["role"], "system");
    assert_eq!(b["messages"][1]["role"], "user");
    assert_eq!(b["messages"][1]["content"], "hi");
}

#[test]
fn parse_openai_reads_content() {
    let v = json!({"choices": [{"finish_reason": "stop", "message": {"content": "  Hi. "}}]});
    assert_eq!(parse_openai(&v).unwrap(), "Hi.");
}

#[test]
fn parse_openai_rejects_length_cutoff() {
    let v = json!({"choices": [{"finish_reason": "length", "message": {"content": "  Hi. "}}]});
    assert!(parse_openai(&v).is_err());
}

#[test]
fn finish_sentence_leaves_punctuated_text_alone() {
    for s in ["Ship it on Friday.", "kya hua?", "wow!", "मैं कल आऊंगा।", "He said \"hi.\"", "", "  "] {
        assert_eq!(finish_sentence(s), s);
    }
}

#[test]
fn finish_sentence_adds_period_and_capital() {
    assert_eq!(finish_sentence("bhai kal milte hain na"), "Bhai kal milte hain na.");
    assert_eq!(finish_sentence("Let's meet at 6 pm "), "Let's meet at 6 pm.");
    // A brand written in lower camel case keeps its first letter.
    assert_eq!(finish_sentence("iPhone wala order aa gaya"), "iPhone wala order aa gaya.");
    // Only the last sentence is checked.
    assert_eq!(finish_sentence("Done. what about lunch"), "Done. What about lunch?");
    assert_eq!(finish_sentence("मैं कल मार्केट जाऊंगा"), "मैं कल मार्केट जाऊंगा।");
}

#[test]
fn finish_sentence_spots_questions() {
    for (s, want) in [
        ("kya tumne PDF bhej di", "Kya tumne PDF bhej di?"),
        ("biryani kaise banate hain", "Biryani kaise banate hain?"),
        ("tum kab aa rahe ho", "Tum kab aa rahe ho?"),
        ("what time is the meeting", "What time is the meeting?"),
        ("Can you ask Karthik to push the changes", "Can you ask Karthik to push the changes?"),
        ("where should we eat tonight", "Where should we eat tonight?"),
        ("क्या तुमने खाना खाया", "क्या तुमने खाना खाया?"),
    ] {
        assert_eq!(finish_sentence(s), want);
    }
    // Statements that start like questions.
    for (s, want) in [
        ("when you get a chance send me the deck", "When you get a chance send me the deck."),
        ("When the meeting ends, ping me", "When the meeting ends, ping me."),
        ("do the dishes", "Do the dishes."),
    ] {
        assert_eq!(finish_sentence(s), want);
    }
}
