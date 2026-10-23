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
    /// No engine could start, so every dictation types the raw transcript.
    fn paused(&self) -> bool {
        false
    }
}

/// Why cleanup was skipped, for the overlay. A fixed message: never a key or raw error text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Skip {
    NoGeminiKey,
    NoClaudeKey,
    OllamaDown,
    TooSlow,
    EngineError,
}

impl Skip {
    pub fn message(self) -> &'static str {
        match self {
            Skip::NoGeminiKey => "Cleanup skipped: no Gemini key",
            Skip::NoClaudeKey => "Cleanup skipped: no Claude key",
            Skip::OllamaDown => "Cleanup skipped: Ollama not running",
            Skip::TooSlow => "Cleanup skipped: took too long",
            Skip::EngineError => "Cleanup skipped: engine error",
        }
    }
}

impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Skip {}

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
        let body = ollama_body(&self.model, system, text, keep_alive);
        post(client().post(format!("{}/api/chat", self.base_url)).json(&body)).await
    }
}

pub fn ollama_body(model: &str, system: &str, text: &str, keep_alive: i64) -> Value {
    json!({
        "model": model, "stream": false, "keep_alive": keep_alive, "think": false,
        // Room for a several-minute transcript plus 200 dictionary hints.
        "options": {"temperature": 0, "num_ctx": 8192},
        "messages": [{"role": "system", "content": system}, {"role": "user", "content": text}],
    })
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
        let reply = self.chat(system, text, -1).await.map_err(|e| {
            if e.downcast_ref::<reqwest::Error>().is_some_and(reqwest::Error::is_connect) { Skip::OllamaDown.into() } else { e }
        })?;
        parse_ollama(&reply)
    }
}

/// Any server speaking OpenAI-style `/v1/chat/completions`: the bundled MLX helper and llama-server.
pub struct OpenAiProvider {
    pub base_url: String,
    pub token: String,
    pub model: String,
}

pub fn openai_body(model: &str, system: &str, text: &str) -> Value {
    json!({
        "model": model, "stream": false, "temperature": 0,
        // Room for a several-minute transcript; a cut-off reply is rejected by `parse_openai`.
        "max_tokens": 4096,
        "chat_template_kwargs": {"enable_thinking": false},
        "messages": [{"role": "system", "content": system}, {"role": "user", "content": text}],
    })
}

pub fn parse_openai(v: &Value) -> anyhow::Result<String> {
    let choice = v.pointer("/choices/0").ok_or_else(|| anyhow!("no choices"))?;
    // Cut-off text must not be inserted as if complete; the raw transcript is used instead.
    if choice.get("finish_reason").and_then(Value::as_str) == Some("length") {
        bail!("cleanup output was cut off");
    }
    let s = choice.pointer("/message/content").and_then(Value::as_str).ok_or_else(|| anyhow!("no message content"))?;
    Ok(s.trim().to_string())
}

#[async_trait]
impl CleanupProvider for OpenAiProvider {
    fn base_timeout(&self) -> Duration {
        Duration::from_secs(8)
    }
    async fn clean(&self, system: &str, text: &str) -> anyhow::Result<String> {
        let body = openai_body(&self.model, system, text);
        let req = client().post(format!("{}/v1/chat/completions", self.base_url)).bearer_auth(&self.token).json(&body);
        parse_openai(&post(req).await?)
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
    match v.get("stop_reason").and_then(Value::as_str) {
        Some("refusal") => bail!("model refused"),
        // Cut-off text must not be inserted as if complete; the raw transcript is used instead.
        Some("max_tokens") => bail!("cleanup output was cut off"),
        _ => {}
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
            "model": "claude-haiku-5-5", "max_tokens": 16000, "output_config": {"effort": "low"},
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
