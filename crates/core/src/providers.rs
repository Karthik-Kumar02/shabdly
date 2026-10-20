//! Cleanup providers. Cloud providers receive only the transcript and the
//! dictionary hints (inside the system prompt).
use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::time::Duration;

#[async_trait]
pub trait CleanupProvider: Send + Sync {
    fn base_timeout(&self) -> Duration;
    async fn clean(&self, system: &str, text: &str) -> anyhow::Result<String>;
}

fn client() -> reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(reqwest::Client::new).clone()
}

async fn post(req: reqwest::RequestBuilder) -> anyhow::Result<Value> {
    let resp = req.send().await?;
    let status = resp.status();
    let body: Value = resp.json().await.context("response is not JSON")?;
    if !status.is_success() {
        // Error bodies never echo the key; keep only the message.
        let msg = body.pointer("/error/message").and_then(Value::as_str).unwrap_or("request failed");
        bail!("{status}: {msg}");
    }
    Ok(body)
}

pub struct OllamaProvider {
    pub base_url: String,
    pub model: String,
}

impl Default for OllamaProvider {
    fn default() -> Self {
        OllamaProvider { base_url: "http://localhost:11434".into(), model: "gemma4:e4b".into() }
    }
}

impl OllamaProvider {
    /// One chat call. `keep_alive` -1 keeps the model loaded, 0 unloads it.
    pub(crate) async fn chat(&self, system: &str, text: &str, keep_alive: i64) -> anyhow::Result<Value> {
        let body = json!({
            "model": self.model, "stream": false, "keep_alive": keep_alive, "think": false,
            "options": {"temperature": 0},
            "messages": [{"role": "system", "content": system}, {"role": "user", "content": text}],
        });
        post(client().post(format!("{}/api/chat", self.base_url)).json(&body)).await
    }
}

pub fn parse_ollama(v: &Value) -> anyhow::Result<String> {
    let s = v.pointer("/message/content").and_then(Value::as_str).ok_or_else(|| anyhow!("no message content"))?;
    Ok(s.trim().to_string())
}

#[async_trait]
impl CleanupProvider for OllamaProvider {
    fn base_timeout(&self) -> Duration {
        Duration::from_secs(8)
    }
    async fn clean(&self, system: &str, text: &str) -> anyhow::Result<String> {
        parse_ollama(&self.chat(system, text, -1).await?)
    }
}

pub struct GeminiProvider {
    pub key: String,
    pub model: String,
}

impl GeminiProvider {
    pub fn new(key: String) -> Self {
        GeminiProvider { key, model: "gemini-flash-lite-latest".into() }
    }
}

pub fn parse_gemini(v: &Value) -> anyhow::Result<String> {
    let parts = v.pointer("/candidates/0/content/parts").and_then(Value::as_array).ok_or_else(|| anyhow!("no candidates"))?;
    let text: String = parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect();
    Ok(text.trim().to_string())
}

#[async_trait]
impl CleanupProvider for GeminiProvider {
    fn base_timeout(&self) -> Duration {
        Duration::from_secs(30)
    }
    async fn clean(&self, system: &str, text: &str) -> anyhow::Result<String> {
        let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", self.model);
        let body = json!({
            "systemInstruction": {"parts": [{"text": system}]},
            "contents": [{"role": "user", "parts": [{"text": text}]}],
            "generationConfig": {"temperature": 0},
        });
        parse_gemini(&post(client().post(url).header("x-goog-api-key", &self.key).json(&body)).await?)
    }
}

pub struct HaikuProvider {
    pub key: String,
}

pub fn parse_anthropic(v: &Value) -> anyhow::Result<String> {
    if v.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        bail!("model refused");
    }
    let blocks = v.get("content").and_then(Value::as_array).ok_or_else(|| anyhow!("no content"))?;
    let text: String = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect();
    Ok(text.trim().to_string())
}

#[async_trait]
impl CleanupProvider for HaikuProvider {
    fn base_timeout(&self) -> Duration {
        Duration::from_secs(30)
    }
    async fn clean(&self, system: &str, text: &str) -> anyhow::Result<String> {
        let body = json!({
            "model": "claude-haiku-5-5", "max_tokens": 1024, "output_config": {"effort": "low"},
            "system": system, "messages": [{"role": "user", "content": text}],
        });
        let req = client()
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.key)
            .header("anthropic-version", "2023-06-01")
            .json(&body);
        parse_anthropic(&post(req).await?)
    }
}
