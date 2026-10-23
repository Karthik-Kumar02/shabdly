use anyhow::Context;
use serde::{Serialize, de::DeserializeOwned};
use std::path::Path;

/// Load JSON from `path`; a missing file gives `T::default()`.
pub fn load<T: DeserializeOwned + Default>(path: &Path) -> anyhow::Result<T> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write `path.tmp`, then rename over `path`, so a crash never leaves half a file.
pub fn save<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Like `load`, but a file that cannot be read is renamed to
/// `<name>.bad-<unix secs>` and the default returned, so the next save never
/// overwrites the user's only copy.
pub fn load_or_set_aside<T: DeserializeOwned + Default>(path: &Path) -> T {
    load(path).unwrap_or_else(|e| {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let mut bad = path.as_os_str().to_owned();
        bad.push(format!(".bad-{secs}"));
        eprintln!("{e:#}; kept the old file as {}", std::path::Path::new(&bad).display());
        let _ = std::fs::rename(path, &bad);
        T::default()
    })
}

/// Copy `settings.json` to `settings.v1.json` once, before this version first rewrites it,
/// so a downgrade to 0.1.5 can restore the original. A file that already has `local_engine` was
/// written by 0.2.0 (backup deleted, or a fresh 0.2.0 install), so it is not a 0.1.5 original.
pub fn backup_v1_once(dir: &Path) {
    let backup = dir.join("settings.v1.json");
    let settings = dir.join("settings.json");
    let v2 = std::fs::read_to_string(&settings)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .is_some_and(|v| v.get("local_engine").is_some());
    if !backup.exists() && !v2 {
        if let Err(e) = std::fs::copy(&settings, &backup) {
            eprintln!("settings backup: {e}");
        }
    }
}
