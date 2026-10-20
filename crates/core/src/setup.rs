//! First-run setup helpers: is the local engine ready, and model downloads.
use anyhow::{Context, bail};
use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OllamaStatus {
    Missing,
    NoModel,
    Ready,
}

/// Read Ollama's `/api/tags` reply. Anything unreadable counts as Ollama missing.
pub fn parse_tags(json: &str, model: &str) -> OllamaStatus {
    let Ok(v) = serde_json::from_str::<Value>(json) else { return OllamaStatus::Missing };
    let Some(models) = v.get("models").and_then(Value::as_array) else { return OllamaStatus::Missing };
    let listed = models.iter().filter_map(|m| m.get("name").and_then(Value::as_str)).any(|n| n == model);
    if listed { OllamaStatus::Ready } else { OllamaStatus::NoModel }
}

/// One line of `/api/pull` progress: (completed, total) bytes when present.
pub fn parse_pull_line(line: &str) -> Option<(u64, u64)> {
    let v: Value = serde_json::from_str(line).ok()?;
    Some((v.get("completed")?.as_u64()?, v.get("total")?.as_u64()?))
}

pub async fn ollama_status(base_url: &str, model: &str) -> OllamaStatus {
    match reqwest::get(format!("{base_url}/api/tags")).await {
        Ok(r) => parse_tags(&r.text().await.unwrap_or_default(), model),
        Err(_) => OllamaStatus::Missing,
    }
}

/// Ask Ollama to download `model`, reporting byte progress.
pub async fn pull(base_url: &str, model: &str, progress: impl Fn(u64, u64)) -> anyhow::Result<()> {
    let mut resp = reqwest::Client::new()
        .post(format!("{base_url}/api/pull"))
        .json(&serde_json::json!({ "model": model, "stream": true }))
        .send()
        .await
        .context("Ollama is not running")?
        .error_for_status()?;
    let mut pending = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        pending.extend_from_slice(&chunk);
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(err) = serde_json::from_str::<Value>(&line).ok().and_then(|v| v["error"].as_str().map(String::from)) {
                bail!("Ollama: {err}");
            }
            if let Some((done, total)) = parse_pull_line(&line) {
                progress(done, total);
            }
        }
    }
    Ok(())
}

/// Download `url` to `dest` via `dest.part`, renamed only when complete, so a
/// failed or interrupted download never leaves a half model behind.
pub async fn download(url: &str, dest: &Path, progress: impl Fn(u64, u64)) -> anyhow::Result<()> {
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = std::path::PathBuf::from(part);
    let result = async {
        let mut resp = reqwest::get(url).await?.error_for_status()?;
        let total = resp.content_length().unwrap_or(0);
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = std::io::BufWriter::new(std::fs::File::create(&part)?);
        let mut done = 0u64;
        while let Some(chunk) = resp.chunk().await? {
            file.write_all(&chunk)?;
            done += chunk.len() as u64;
            progress(done, total.max(done));
        }
        file.flush()?;
        if total > 0 && done != total {
            bail!("download stopped at {done} of {total} bytes");
        }
        std::fs::rename(&part, dest)?;
        anyhow::Ok(())
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}
