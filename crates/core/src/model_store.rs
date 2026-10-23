//! Finding and verifying cleanup models (spec section 3): Shabdly's own folder first, then
//! the Hugging Face cache, checked by size and sha256. A verified cache copy is cloned in so
//! clearing the cache never breaks Shabdly.
use crate::models::{ModelFile, ModelSpec};
use anyhow::Context;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelState {
    Ready(PathBuf),
    Missing { bytes_needed: u64 },
}

/// `verified.json`: file name to `[size, mtime_secs]` at the time its sha256 matched.
type Verified = BTreeMap<String, (u64, u64)>;

pub fn model_dir(models_dir: &Path, spec: &ModelSpec) -> PathBuf {
    models_dir.join(spec.id)
}

/// Where `huggingface_hub` keeps its cache, in its own order.
pub fn hf_hub_dir() -> Option<PathBuf> {
    hf_hub_dir_from(|k| std::env::var_os(k))
}

/// `$HF_HUB_CACHE`, else `$HF_HOME/hub`, else `$XDG_CACHE_HOME/huggingface/hub`, else
/// `<home>/.cache/huggingface/hub`. Empty values count as unset.
pub fn hf_hub_dir_from(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let var = |k| var(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(h) = var("HF_HUB_CACHE") {
        return Some(h);
    }
    if let Some(h) = var("HF_HOME") {
        return Some(h.join("hub"));
    }
    let cache = var("XDG_CACHE_HOME").or_else(|| Some(var("HOME").or_else(|| var("USERPROFILE"))?.join(".cache")))?;
    Some(cache.join("huggingface/hub"))
}

pub fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(format!("{:x}", h.finalize()))
}

/// Size (following symlinks), or None if absent.
fn size_of(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len())
}

fn has_size(dir: &Path, f: &ModelFile) -> bool {
    size_of(&dir.join(f.name)) == Some(f.size)
}

fn stamp(path: &Path) -> anyhow::Result<(u64, u64)> {
    let m = fs::metadata(path)?;
    let mtime = m
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok((m.len(), mtime))
}

pub(crate) fn write_verified(dir: &Path, spec: &ModelSpec) -> anyhow::Result<()> {
    let mut v = Verified::new();
    for f in spec.files {
        v.insert(f.name.to_string(), stamp(&dir.join(f.name))?);
    }
    fs::write(dir.join("verified.json"), serde_json::to_vec(&v)?)?;
    Ok(())
}

fn verified_matches(dir: &Path, spec: &ModelSpec) -> bool {
    let Ok(bytes) = fs::read(dir.join("verified.json")) else {
        return false;
    };
    let Ok(v) = serde_json::from_slice::<Verified>(&bytes) else {
        return false;
    };
    spec.files.iter().all(|f| {
        v.get(f.name)
            .is_some_and(|s| stamp(&dir.join(f.name)).ok().as_ref() == Some(s))
    })
}

/// `models--<org>--<name>` from a `https://huggingface.co/<org>/<name>/resolve/...` URL.
fn hf_repo_folder(url: &str) -> Option<String> {
    let mut parts = url.strip_prefix("https://huggingface.co/")?.split('/');
    Some(format!("models--{}--{}", parts.next()?, parts.next()?))
}

/// A snapshot folder holding every file with the right size and sha256, if any.
fn find_hf_snapshot(spec: &ModelSpec, hub: &Path) -> Option<PathBuf> {
    let repo = hf_repo_folder(spec.files.first()?.url)?;
    let snapshots = fs::read_dir(hub.join(repo).join("snapshots")).ok()?;
    snapshots.flatten().map(|e| e.path()).find(|snap| {
        spec.files.iter().all(|f| has_size(snap, f))
            && spec
                .files
                .iter()
                .all(|f| sha256_file(&snap.join(f.name)).is_ok_and(|h| h == f.sha256))
    })
}

pub fn locate(
    spec: &ModelSpec,
    models_dir: &Path,
    hf_hub: Option<&Path>,
) -> anyhow::Result<ModelState> {
    let dir = model_dir(models_dir, spec);

    // 1. Own folder: sizes first, then verified.json or a full hash.
    if spec.files.iter().all(|f| has_size(&dir, f)) {
        if verified_matches(&dir, spec) {
            return Ok(ModelState::Ready(dir));
        }
        let mut all_ok = true;
        for f in spec.files {
            let path = dir.join(f.name);
            if sha256_file(&path)? != f.sha256 {
                fs::remove_file(&path)?;
                all_ok = false;
            }
        }
        if all_ok {
            write_verified(&dir, spec)?;
            return Ok(ModelState::Ready(dir));
        }
    }

    // 2. Hugging Face cache. fs::copy reads through symlinks into blobs/ and clones on APFS.
    if let Some(snap) = hf_hub.and_then(|hub| find_hf_snapshot(spec, hub)) {
        for f in spec.files {
            let dst = dir.join(f.name);
            fs::create_dir_all(dst.parent().unwrap_or(&dir))?;
            let _ = fs::remove_file(&dst);
            fs::copy(snap.join(f.name), &dst)?;
        }
        if spec.files.iter().all(|f| has_size(&dir, f)) {
            write_verified(&dir, spec)?;
            return Ok(ModelState::Ready(dir));
        }
    }

    // 3. Whatever is not already present with the right size must be downloaded.
    let bytes_needed = spec
        .files
        .iter()
        .filter(|f| !has_size(&dir, f))
        .map(|f| f.size)
        .sum();
    Ok(ModelState::Missing { bytes_needed })
}
