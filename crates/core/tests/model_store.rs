use shabdly_core::model_store::{ModelState, hf_hub_dir_from, locate, model_dir, sha256_file};
use shabdly_core::models::{EngineKind, ModelFile, ModelSpec};
use shabdly_core::settings::Speed;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const A: &[u8] = b"hello";
const B: &[u8] = b"world!";
const A_SHA: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
const B_SHA: &str = "711e9609339e92b03ddc0a211827dba421f38f9ed8b9d806e1ffdd8c15ffa03d";
const REV: &str = "0123456789abcdef0123456789abcdef01234567";

static FILES: [ModelFile; 2] = [
    ModelFile {
        name: "a.bin",
        size: 5,
        sha256: A_SHA,
        url: "https://huggingface.co/org/name/resolve/0123456789abcdef0123456789abcdef01234567/a.bin",
    },
    ModelFile {
        name: "b.bin",
        size: 6,
        sha256: B_SHA,
        url: "https://huggingface.co/org/name/resolve/0123456789abcdef0123456789abcdef01234567/b.bin",
    },
];

static SPEC: ModelSpec = ModelSpec {
    id: "test-model",
    engine: EngineKind::Llama,
    speed: Speed::Fast,
    files: &FILES,
    ram_gb: 1.0,
};

fn fill(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("a.bin"), A).unwrap();
    fs::write(dir.join("b.bin"), B).unwrap();
}

fn snapshot(hub: &Path) -> PathBuf {
    hub.join("models--org--name/snapshots").join(REV)
}

/// Overwrites `path` with `content`, then sets its modification time to `mtime`.
fn rewrite(path: &Path, content: &[u8], mtime: std::time::SystemTime) {
    fs::write(path, content).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

#[test]
fn ready_when_own_folder_matches() {
    let t = tempfile::tempdir().unwrap();
    let dir = model_dir(t.path(), &SPEC);
    assert_eq!(dir, t.path().join("test-model"));
    fill(&dir);
    assert_eq!(
        locate(&SPEC, t.path(), None).unwrap(),
        ModelState::Ready(dir.clone())
    );
    assert!(dir.join("verified.json").is_file());
    assert_eq!(sha256_file(&dir.join("a.bin")).unwrap(), A_SHA);
}

#[test]
fn missing_file_means_missing() {
    let t = tempfile::tempdir().unwrap();
    let dir = model_dir(t.path(), &SPEC);
    fill(&dir);
    fs::remove_file(dir.join("b.bin")).unwrap();
    assert_eq!(
        locate(&SPEC, t.path(), None).unwrap(),
        ModelState::Missing { bytes_needed: 6 }
    );
}

#[test]
fn wrong_size_means_missing() {
    let t = tempfile::tempdir().unwrap();
    let dir = model_dir(t.path(), &SPEC);
    fill(&dir);
    fs::write(dir.join("b.bin"), b"x").unwrap();
    assert_eq!(
        locate(&SPEC, t.path(), None).unwrap(),
        ModelState::Missing { bytes_needed: 6 }
    );
}

#[test]
fn wrong_hash_deletes_file_and_reports_missing() {
    let t = tempfile::tempdir().unwrap();
    let dir = model_dir(t.path(), &SPEC);
    fill(&dir);
    fs::write(dir.join("b.bin"), b"WORLD!").unwrap();
    assert_eq!(
        locate(&SPEC, t.path(), None).unwrap(),
        ModelState::Missing { bytes_needed: 6 }
    );
    assert!(!dir.join("b.bin").exists());
    assert!(dir.join("a.bin").exists());
}

#[test]
fn verified_json_skips_rehash() {
    let t = tempfile::tempdir().unwrap();
    let dir = model_dir(t.path(), &SPEC);
    fill(&dir);
    assert!(matches!(
        locate(&SPEC, t.path(), None).unwrap(),
        ModelState::Ready(_)
    ));
    let b = dir.join("b.bin");
    let mtime = fs::metadata(&b).unwrap().modified().unwrap();
    rewrite(&b, b"WORLD!", mtime);
    assert_eq!(
        locate(&SPEC, t.path(), None).unwrap(),
        ModelState::Ready(dir)
    );
}

#[test]
fn verified_json_invalidated_by_mtime() {
    let t = tempfile::tempdir().unwrap();
    let dir = model_dir(t.path(), &SPEC);
    fill(&dir);
    assert!(matches!(
        locate(&SPEC, t.path(), None).unwrap(),
        ModelState::Ready(_)
    ));
    let b = dir.join("b.bin");
    let mtime = fs::metadata(&b).unwrap().modified().unwrap();
    rewrite(&b, b"WORLD!", mtime + Duration::from_secs(10));
    assert_eq!(
        locate(&SPEC, t.path(), None).unwrap(),
        ModelState::Missing { bytes_needed: 6 }
    );
}

#[test]
fn hf_snapshot_is_cloned_in() {
    let t = tempfile::tempdir().unwrap();
    let (models, hub) = (t.path().join("models"), t.path().join("hub"));
    fill(&snapshot(&hub));
    let dir = model_dir(&models, &SPEC);
    assert_eq!(
        locate(&SPEC, &models, Some(&hub)).unwrap(),
        ModelState::Ready(dir.clone())
    );
    assert_eq!(fs::read(dir.join("a.bin")).unwrap(), A);
    assert_eq!(fs::read(dir.join("b.bin")).unwrap(), B);
    assert!(dir.join("verified.json").is_file());
}

#[cfg(unix)]
#[test]
fn hf_snapshot_symlinks_are_followed() {
    use std::os::unix::fs::symlink;
    let t = tempfile::tempdir().unwrap();
    let (models, hub) = (t.path().join("models"), t.path().join("hub"));
    let blobs = hub.join("models--org--name/blobs");
    fs::create_dir_all(&blobs).unwrap();
    fs::write(blobs.join(A_SHA), A).unwrap();
    fs::write(blobs.join(B_SHA), B).unwrap();
    let snap = snapshot(&hub);
    fs::create_dir_all(&snap).unwrap();
    symlink(format!("../../blobs/{A_SHA}"), snap.join("a.bin")).unwrap();
    symlink(format!("../../blobs/{B_SHA}"), snap.join("b.bin")).unwrap();

    let dir = model_dir(&models, &SPEC);
    assert_eq!(
        locate(&SPEC, &models, Some(&hub)).unwrap(),
        ModelState::Ready(dir.clone())
    );
    for name in ["a.bin", "b.bin"] {
        assert!(
            fs::symlink_metadata(dir.join(name)).unwrap().is_file(),
            "{name} is a symlink"
        );
    }
    fs::remove_dir_all(&hub).unwrap();
    assert_eq!(fs::read(dir.join("a.bin")).unwrap(), A);
    assert_eq!(fs::read(dir.join("b.bin")).unwrap(), B);
}

#[test]
fn partial_hf_snapshot_is_ignored() {
    let t = tempfile::tempdir().unwrap();
    let (models, hub) = (t.path().join("models"), t.path().join("hub"));
    let snap = snapshot(&hub);
    fs::create_dir_all(&snap).unwrap();
    fs::write(snap.join("a.bin"), A).unwrap();
    assert_eq!(
        locate(&SPEC, &models, Some(&hub)).unwrap(),
        ModelState::Missing { bytes_needed: 11 }
    );
    assert!(!model_dir(&models, &SPEC).join("a.bin").exists());
}

#[test]
fn hf_hub_dir_follows_huggingface_order() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.into())
    };
    const ALL: &[(&str, &str)] = &[("HF_HUB_CACHE", "/c"), ("HF_HOME", "/h"), ("XDG_CACHE_HOME", "/x"), ("HOME", "/u")];
    assert_eq!(hf_hub_dir_from(env(ALL)), Some(PathBuf::from("/c")));
    assert_eq!(hf_hub_dir_from(env(&ALL[1..])), Some(PathBuf::from("/h/hub")));
    assert_eq!(hf_hub_dir_from(env(&ALL[2..])), Some(PathBuf::from("/x/huggingface/hub")));
    assert_eq!(hf_hub_dir_from(env(&[("XDG_CACHE_HOME", ""), ("HOME", "/u")])), Some(PathBuf::from("/u/.cache/huggingface/hub")));
}
