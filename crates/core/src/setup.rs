//! First-run setup helpers: is the local engine ready, and model downloads.
use crate::model_store::{model_dir, sha256_file, write_verified};
use crate::models::{ModelSpec, mirror_url, total_bytes};
use anyhow::{Context, anyhow, bail};
use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DOWNLOAD_FAILED: &str = "Download failed. Check your internet and try again";
const BACKOFF: [Duration; 3] = [Duration::from_secs(2), Duration::from_secs(8), Duration::from_secs(30)];

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

fn part_of(dest: &Path) -> PathBuf {
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    PathBuf::from(part)
}

fn len_of(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

async fn sha256_async(path: &Path) -> anyhow::Result<String> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || sha256_file(&path)).await?
}

/// One GET of `url` into `part`, resuming from what is already there when the server allows it.
async fn fetch(url: &str, part: &Path, size: u64, progress: &impl Fn(u64, u64)) -> anyhow::Result<()> {
    let have = len_of(part);
    // A stalled connection fails after 30 s of silence instead of hanging setup forever.
    let mut req = reqwest::Client::builder().read_timeout(Duration::from_secs(30)).build()?.get(url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut resp = req.send().await?.error_for_status()?;
    // 206 continues the file; a 200 is the whole file again, so start over.
    let resume = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let file = std::fs::OpenOptions::new().create(true).write(true).append(resume).truncate(!resume).open(part)?;
    let mut file = std::io::BufWriter::new(file);
    let mut done = if resume { have } else { 0 };
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk)?;
        done += chunk.len() as u64;
        progress(done, size);
    }
    file.flush()?;
    if done < size {
        bail!("download stopped at {done} of {size} bytes");
    }
    Ok(())
}

/// Each URL in turn: a first try plus one retry per `backoff` delay.
async fn fetch_any(urls: &[String], part: &Path, size: u64, backoff: &[Duration], progress: &impl Fn(u64, u64)) -> anyhow::Result<()> {
    let mut last = anyhow!("no download URL");
    for url in urls {
        for wait in std::iter::once(&Duration::ZERO).chain(backoff) {
            tokio::time::sleep(*wait).await;
            match fetch(url, part, size, progress).await {
                Ok(()) => return Ok(()),
                // Local disk trouble (full, read-only): retrying cannot help; `.part` stays for Retry.
                Err(e) if e.is::<std::io::Error>() => return Err(e),
                Err(e) => last = e,
            }
        }
    }
    Err(last.context(DOWNLOAD_FAILED))
}

/// Download one model file to `dest` through `dest.part`: resume with Range, retry with
/// `backoff`, fall back to later URLs, and rename only after size and sha256 match.
/// A bad hash deletes `.part` and downloads once more; a second bad hash is an error.
pub async fn download_file(
    urls: &[String],
    dest: &Path,
    size: u64,
    sha256: &str,
    backoff: &[Duration],
    progress: impl Fn(u64, u64),
) -> anyhow::Result<()> {
    let part = part_of(dest);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    for _ in 0..2 {
        // A `.part` already at full size (quit while hashing) only needs checking.
        if len_of(&part) < size {
            fetch_any(urls, &part, size, backoff, &progress).await?;
        }
        if len_of(&part) == size && sha256_async(&part).await? == sha256 {
            std::fs::rename(&part, dest)?;
            return Ok(());
        }
        std::fs::remove_file(&part)?;
    }
    bail!(DOWNLOAD_FAILED)
}

/// Download whatever `spec` still lacks into its folder, progress summed over the whole
/// model, then record it in `verified.json`. Files already there are hashed, not re-fetched.
pub async fn download_model(spec: &ModelSpec, models_dir: &Path, progress: impl Fn(u64, u64)) -> anyhow::Result<()> {
    let dir = model_dir(models_dir, spec);
    let total = total_bytes(spec);
    let progress = &progress;
    let mut base = 0;
    for f in spec.files {
        let dest = dir.join(f.name);
        let present = len_of(&dest) == f.size && sha256_async(&dest).await? == f.sha256;
        if !present {
            let urls = [f.url.to_string(), mirror_url(spec, f)];
            download_file(&urls, &dest, f.size, f.sha256, &BACKOFF, move |done, _| progress(base + done, total)).await?;
        }
        base += f.size;
        progress(base, total);
    }
    write_verified(&dir, spec)
}
