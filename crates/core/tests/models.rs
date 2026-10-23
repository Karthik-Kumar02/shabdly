use shabdly_core::asr::WHISPER_MODELS;
use shabdly_core::models::{CATALOG, EngineKind, MIRROR_BASE, Os, DEFAULT_SPEED, best_offered, mirror_url, pick};
use shabdly_core::settings::Speed;

#[test]
fn fast_is_default_and_ram_gates_best() {
    assert_eq!(DEFAULT_SPEED, Speed::Fast);
    assert!(!best_offered(8, true));
    assert!(!best_offered(15, true));
    assert!(best_offered(16, true));
    assert!(!best_offered(32, false));
}

#[test]
fn mac_14_gets_mlx() {
    assert_eq!(pick(Os::Mac, (14, 0), Speed::Best).unwrap().engine, EngineKind::Mlx);
    assert_eq!(pick(Os::Mac, (15, 2), Speed::Fast).unwrap().engine, EngineKind::Mlx);
}

#[test]
fn mac_13_3_gets_llama() {
    assert_eq!(pick(Os::Mac, (13, 3), Speed::Best).unwrap().engine, EngineKind::Llama);
    assert_eq!(pick(Os::Mac, (13, 6), Speed::Fast).unwrap().engine, EngineKind::Llama);
}

#[test]
fn mac_below_13_3_has_no_bundled_engine() {
    assert!(pick(Os::Mac, (13, 2), Speed::Fast).is_none());
    assert!(pick(Os::Mac, (12, 7), Speed::Fast).is_none());
    assert!(pick(Os::Mac, (11, 0), Speed::Best).is_none());
}

#[test]
fn windows_gets_llama() {
    for speed in [Speed::Best, Speed::Fast] {
        let spec = pick(Os::Windows, (0, 0), speed).unwrap();
        assert_eq!(spec.engine, EngineKind::Llama);
        assert_eq!(spec.speed, speed);
    }
}

#[test]
fn catalog_is_well_formed() {
    let mut ids: Vec<_> = CATALOG.iter().map(|s| s.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), CATALOG.len(), "duplicate ids");
    for spec in CATALOG {
        assert!(!spec.files.is_empty(), "{} has no files", spec.id);
        for f in spec.files {
            let hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);
            assert!(
                f.sha256.len() == 64 && f.sha256.chars().all(hex),
                "{} {}",
                spec.id,
                f.name
            );
            assert!(f.size > 0, "{} {}", spec.id, f.name);
            let rev = f
                .url
                .split("/resolve/")
                .nth(1)
                .and_then(|r| r.split('/').next())
                .unwrap_or("");
            assert!(
                rev.len() == 40 && rev.chars().all(hex),
                "{} unpinned url {}",
                spec.id,
                f.url
            );
            assert!(
                f.url.ends_with(&format!("/{}", f.name)),
                "{} url {}",
                spec.id,
                f.url
            );
        }
    }
}

#[test]
fn mirror_url_format() {
    let spec = CATALOG
        .iter()
        .find(|s| s.id == "mlx-gemma4-e4b-5bit")
        .unwrap();
    let f = spec
        .files
        .iter()
        .find(|f| f.name == "model-00001-of-00002.safetensors")
        .unwrap();
    assert_eq!(
        mirror_url(spec, f),
        format!("{MIRROR_BASE}/mlx-gemma4-e4b-5bit/model-00001-of-00002.safetensors")
    );
}

/// Whisper downloads like the engine models (resume, retry, sha256), so its table must be
/// pinned the same way: a fixed commit, a size and a sha256 per file.
#[test]
fn whisper_models_are_pinned() {
    let hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);
    for f in &WHISPER_MODELS {
        assert!(f.sha256.len() == 64 && f.sha256.chars().all(hex), "{}", f.name);
        assert!(f.size > 0, "{}", f.name);
        let rev = f.url.split("/resolve/").nth(1).and_then(|r| r.split('/').next()).unwrap_or("");
        assert!(rev.len() == 40 && rev.chars().all(hex), "unpinned url {}", f.url);
        assert!(f.url.ends_with(&format!("/{}", f.name)), "{}", f.url);
    }
}
