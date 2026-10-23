//! One dictation: speech to text, cleanup, insertion, then watching the field
//! to learn from the user's corrections (spec s3, s4).
use crate::capture::Recorder;
use crate::gesture::{Action, Gesture, Input};
use crate::hotkey::HotkeyEvent;
use crate::observe::observe;
use crate::platform::{PasteOutcome, Platform, insert_into};
use serde_json::{Value, json};
use shabdly_core::asr::Transcriber;
use shabdly_core::audio::{rms, to_16k_mono};
use shabdly_core::dictionary::Dictionary;
use shabdly_core::history::{History, HistoryItem};
use shabdly_core::learning::{LearnEvent, learn};
use shabdly_core::pipeline::clean;
use shabdly_core::providers::{CleanupProvider, Skip};
use shabdly_core::settings::Settings;
use shabdly_core::store;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Below this RMS the recording is treated as silence (Whisper invents text for silence).
const SILENCE_RMS: f32 = 0.002;

pub const MIC_BLOCKED: &str = if cfg!(target_os = "macos") {
    "Shabdly can't hear your microphone. Allow it in System Settings, Privacy & Security, Microphone."
} else {
    "Shabdly can't hear your microphone. Check that it is connected and allowed in Privacy settings."
};

pub struct Deps {
    pub platform: Arc<dyn Platform>,
    pub asr: Arc<dyn Transcriber>,
    pub provider: Arc<dyn CleanupProvider>,
    pub dict: Arc<Mutex<Dictionary>>,
    pub history: Arc<Mutex<History>>,
    pub settings: Arc<Mutex<Settings>>,
    /// Where dictionary.json and history.json are saved; None in tests.
    pub data_dir: Option<PathBuf>,
    pub poll: Duration,
    pub max_observe: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Inserted { text: String, skipped: Option<Skip>, at_ms: i64, element_id: u64 },
    Copied { text: String },
    Blocked { text: String },
    Empty,
    Failed { message: String },
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Deps {
    pub(crate) fn save_dict(&self) {
        if let Some(dir) = &self.data_dir {
            let d = self.dict.lock().unwrap().clone();
            if let Err(e) = store::save(&dir.join("dictionary.json"), &d) {
                eprintln!("saving dictionary: {e:#}");
            }
        }
    }

    pub(crate) fn save_history(&self) {
        if let Some(dir) = &self.data_dir {
            let h = self.history.lock().unwrap().clone();
            if let Err(e) = store::save(&dir.join("history.json"), &h) {
                eprintln!("saving history: {e:#}");
            }
        }
    }
}

/// Speech to inserted text.
pub async fn process(deps: &Deps, samples_16k: Vec<f32>) -> Outcome {
    // A real microphone always has some noise; exact zeros mean the OS is
    // withholding audio (permission denied on macOS).
    if !samples_16k.is_empty() && samples_16k.iter().all(|s| *s == 0.0) {
        return Outcome::Failed { message: MIC_BLOCKED.into() };
    }
    if samples_16k.is_empty() || rms(&samples_16k) < SILENCE_RMS {
        return Outcome::Empty;
    }
    let settings = deps.settings.lock().unwrap().clone();
    let asr = deps.asr.clone();
    let audio_ms = samples_16k.len() / 16;
    let t = Instant::now();
    let raw = match tokio::task::spawn_blocking(move || asr.transcribe(&samples_16k, settings.language)).await {
        Ok(Ok(t)) => t.trim().to_string(),
        Ok(Err(e)) => return Outcome::Failed { message: format!("Speech recognition failed: {e}") },
        Err(e) => return Outcome::Failed { message: format!("Speech recognition failed: {e}") },
    };
    let asr_ms = t.elapsed().as_millis();
    if raw.is_empty() {
        return Outcome::Empty;
    }
    let dict = deps.dict.lock().unwrap().clone();
    let t = Instant::now();
    let cleaned = clean(&raw, &dict, settings.language, deps.provider.as_ref()).await;
    let clean_ms = t.elapsed().as_millis();
    if !cleaned.used_words.is_empty() {
        let now = now_ms() / 1000;
        let mut d = deps.dict.lock().unwrap();
        for w in &cleaned.used_words {
            d.record_use(w, now);
        }
        drop(d);
        deps.save_dict();
    }

    let platform = deps.platform.clone();
    let text = cleaned.text.clone();
    let t = Instant::now();
    let (app, (pasted, field)) = tokio::task::spawn_blocking(move || {
        let app = platform.frontmost_app();
        (app, insert_into(platform.as_ref(), &text))
    })
    .await
    .unwrap_or((None, (PasteOutcome::Copied, None)));
    // Stage times only, never the transcript.
    eprintln!(
        "timing: audio {audio_ms} ms, asr {asr_ms} ms, cleanup {clean_ms} ms{}, insert {} ms, words {}",
        if cleaned.skipped.is_some() { " (skipped)" } else { "" },
        t.elapsed().as_millis(),
        raw.split_whitespace().count()
    );

    let at_ms = now_ms();
    if settings.keep_history {
        deps.history.lock().unwrap().push(HistoryItem {
            at_ms,
            raw,
            cleaned: cleaned.text.clone(),
            final_text: None,
            app,
        });
        deps.save_history();
    }
    let text = cleaned.text;
    match (pasted, field) {
        (PasteOutcome::Pasted, Some(f)) => {
            Outcome::Inserted { text, skipped: cleaned.skipped, at_ms, element_id: f.element_id }
        }
        (PasteOutcome::Blocked, _) => Outcome::Blocked { text },
        _ => Outcome::Copied { text },
    }
}

/// The part of `final_text` that replaced `inserted`, judged against the
/// field right after the paste. None when the user edited outside it.
fn inserted_region<'a>(baseline: &str, final_text: &'a str, inserted: &str) -> Option<&'a str> {
    let pos = baseline.rfind(inserted)?;
    let (prefix, suffix) = (&baseline[..pos], &baseline[pos + inserted.len()..]);
    final_text.strip_prefix(prefix)?.strip_suffix(suffix)
}

/// Watch the field after an insertion and learn from the user's edits.
pub async fn learn_after(
    deps: &Deps,
    inserted: String,
    at_ms: i64,
    element_id: u64,
    cancel: Arc<AtomicBool>,
    enter: Receiver<()>,
) -> Vec<LearnEvent> {
    let settings = deps.settings.lock().unwrap().clone();
    if !settings.learn_from_corrections && !settings.keep_history {
        return vec![];
    }
    let platform = deps.platform.clone();
    let (poll, max) = (deps.poll, deps.max_observe);
    let seen = tokio::task::spawn_blocking(move || {
        // The field right after the paste is the baseline the edits are compared to.
        let baseline =
            platform.focused_field().filter(|f| f.element_id == element_id && !f.value.is_empty())?.value;
        let last = observe(platform.as_ref(), element_id, &cancel, &enter, poll, max)?;
        Some((baseline, last))
    })
    .await
    .ok()
    .flatten();
    let Some((baseline, last)) = seen else { return vec![] };

    let inserted = inserted.trim();
    let mut events = vec![];
    if let Some(region) = inserted_region(&baseline, &last, inserted) {
        if settings.keep_history {
            deps.history.lock().unwrap().set_final(at_ms, region.trim().to_string());
            deps.save_history();
        }
        if settings.learn_from_corrections {
            events = learn(&mut deps.dict.lock().unwrap(), inserted, region, now_ms() / 1000);
            if !events.is_empty() {
                deps.save_dict();
            }
        }
    }
    events
}

/// Emits a Tauri event (name, payload); a plain function so the flow stays testable.
pub type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

fn overlay(emit: &Emit, state: &str, message: Option<&str>) {
    emit("overlay-state", json!({ "state": state, "message": message }));
}

/// Wires hotkey events to recording, processing, insertion and learning.
pub struct Controller;

impl Controller {
    /// Runs on its own thread for the life of the app. `warm` is false until the
    /// local model has loaded once. `rt` runs the async pipeline.
    pub fn spawn(
        deps: Arc<Deps>,
        hotkeys: Receiver<HotkeyEvent>,
        emit: Emit,
        warm: Arc<AtomicBool>,
        rt: tokio::runtime::Handle,
    ) {
        let (level_tx, level_rx) = channel::<f32>();
        let level_emit = emit.clone();
        std::thread::spawn(move || {
            for level in level_rx {
                level_emit("overlay-state", json!({ "state": "recording", "level": level }));
            }
        });
        std::thread::Builder::new()
            .name("dictation".into())
            .spawn(move || run(deps, hotkeys, emit, warm, rt, level_tx))
            .expect("spawn dictation thread");
    }
}

fn run(
    deps: Arc<Deps>,
    hotkeys: Receiver<HotkeyEvent>,
    emit: Emit,
    warm: Arc<AtomicBool>,
    rt: tokio::runtime::Handle,
    level_tx: Sender<f32>,
) {
    let mut gesture = Gesture::default();
    let mut recorder = Recorder::default();
    // The observation of the previous dictation: its cancel flag and Enter line.
    let watching: Arc<Mutex<Option<(Arc<AtomicBool>, Sender<()>)>>> = Default::default();
    // One dictation is processed at a time, in order.
    let pipeline = Arc::new(tokio::sync::Mutex::new(()));
    // Counts recordings started; a dictation that finishes after a newer one began skips observing.
    let starts = Arc::new(std::sync::atomic::AtomicU64::new(0));
    loop {
        let input = match hotkeys.recv_timeout(Duration::from_millis(100)) {
            Ok(HotkeyEvent::Down) => Input::Down(Instant::now()),
            Ok(HotkeyEvent::Up) => Input::Up(Instant::now()),
            Ok(HotkeyEvent::Esc) => Input::Esc,
            Ok(HotkeyEvent::Enter) => {
                if let Some((_, tx)) = watching.lock().unwrap().as_ref() {
                    let _ = tx.send(());
                }
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Input::Tick(Instant::now()),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        match gesture.feed(input) {
            Action::None => {}
            Action::Start => {
                starts.fetch_add(1, Ordering::SeqCst);
                if let Some((cancel, _)) = watching.lock().unwrap().take() {
                    cancel.store(true, Ordering::SeqCst);
                }
                match recorder.start(level_tx.clone()) {
                    Ok(()) => overlay(&emit, "recording", None),
                    Err(e) => overlay(&emit, "error", Some(&format!("{e}"))),
                }
            }
            Action::Cancel => {
                recorder.stop();
                overlay(&emit, "hidden", None);
            }
            Action::Stop => {
                let (samples, channels, rate) = recorder.stop();
                let released = Instant::now();
                let mine = starts.load(Ordering::SeqCst);
                let (deps, emit, warm, watching, pipeline, starts) =
                    (deps.clone(), emit.clone(), warm.clone(), watching.clone(), pipeline.clone(), starts.clone());
                rt.spawn(async move {
                    let _turn = pipeline.lock().await;
                    let state = if warm.load(Ordering::SeqCst) { "processing" } else { "warming" };
                    overlay(&emit, state, None);
                    let samples = to_16k_mono(&samples, channels, rate);
                    eprintln!("timing: release to asr start {} ms", released.elapsed().as_millis());
                    let outcome = process(&deps, samples).await;
                    eprintln!("timing: release to done {} ms", released.elapsed().as_millis());
                    warm.store(true, Ordering::SeqCst);
                    let (at_ms, element_id, text) = match outcome {
                        Outcome::Inserted { text, skipped, at_ms, element_id } => {
                            match skipped {
                                Some(_) if deps.provider.paused() => {
                                    overlay(&emit, "error", Some("Cleanup paused, typing raw text"))
                                }
                                Some(why) => overlay(&emit, "error", Some(why.message())),
                                None => overlay(&emit, "hidden", None),
                            }
                            (at_ms, element_id, text)
                        }
                        Outcome::Copied { .. } => return overlay(&emit, "error", Some("Copied, paste it anywhere")),
                        Outcome::Blocked { .. } => {
                            return overlay(&emit, "error", Some("Can't type into password fields"));
                        }
                        Outcome::Empty => return overlay(&emit, "hidden", None),
                        Outcome::Failed { message } => return overlay(&emit, "error", Some(&message)),
                    };
                    drop(_turn);
                    if starts.load(Ordering::SeqCst) != mine {
                        return;
                    }
                    let cancel = Arc::new(AtomicBool::new(false));
                    let (enter_tx, enter_rx) = channel();
                    *watching.lock().unwrap() = Some((cancel.clone(), enter_tx));
                    let events = learn_after(&deps, text, at_ms, element_id, cancel, enter_rx).await;
                    if !events.is_empty() {
                        emit("learned", json!({ "events": events }));
                    }
                });
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::platform::fake::FakePlatform;
    use crate::engine::SwitchProvider;
    use crate::local_engine::{Candidate, LocalEngine, Running};
    use crate::platform::{ClipboardSnapshot, FieldText};
    use async_trait::async_trait;
    use shabdly_core::providers::{OllamaProvider, Skip};
    use shabdly_core::settings::{Language, LocalEngineKind};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Instant;

    pub(crate) struct FakeAsr(pub &'static str);
    impl Transcriber for FakeAsr {
        fn transcribe(&self, _s: &[f32], _l: Language) -> anyhow::Result<String> {
            Ok(self.0.into())
        }
    }

    pub(crate) struct Echo;
    #[async_trait]
    impl CleanupProvider for Echo {
        fn base_timeout(&self) -> Duration {
            Duration::from_millis(200)
        }
        async fn clean(&self, _s: &str, text: &str) -> anyhow::Result<String> {
            Ok(text.into())
        }
    }

    struct Down;
    #[async_trait]
    impl CleanupProvider for Down {
        fn base_timeout(&self) -> Duration {
            Duration::from_millis(200)
        }
        async fn clean(&self, _s: &str, _t: &str) -> anyhow::Result<String> {
            anyhow::bail!("connection refused")
        }
    }

    fn field(v: &str, id: u64) -> Option<FieldText> {
        Some(FieldText { value: v.into(), is_password: false, element_id: id })
    }

    pub(crate) fn deps(p: Arc<FakePlatform>, asr: &'static str, provider: Arc<dyn CleanupProvider>) -> Deps {
        Deps {
            platform: p,
            asr: Arc::new(FakeAsr(asr)),
            provider,
            dict: Default::default(),
            history: Default::default(),
            settings: Default::default(),
            data_dir: None,
            poll: Duration::from_millis(5),
            max_observe: Duration::from_secs(2),
        }
    }

    /// Loud enough to pass the silence gate.
    fn speech() -> Vec<f32> {
        (0..16_000).map(|i| ((i as f32) * 0.05).sin() * 0.3).collect()
    }

    fn enter_now() -> std::sync::mpsc::Receiver<()> {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(()).unwrap();
        rx
    }

    fn inserted(o: &Outcome) -> (i64, u64, String) {
        match o {
            Outcome::Inserted { text, at_ms, element_id, .. } => (*at_ms, *element_id, text.clone()),
            other => panic!("expected Inserted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn correction_is_learned_and_used_next_time() {
        let p = Arc::new(FakePlatform::new(vec![field("", 1), field("ask kartik now", 1), field("ask kartik now", 1), field("ask Karthik now", 1)]));
        let d = deps(p.clone(), "ask kartik now", Arc::new(Echo));
        let o = process(&d, speech()).await;
        let (at, id, text) = inserted(&o);
        let events = learn_after(&d, text, at, id, Arc::new(AtomicBool::new(false)), enter_now()).await;
        assert_eq!(events.len(), 1);
        assert_eq!(d.dict.lock().unwrap().get("Karthik").unwrap().heard_as, vec!["kartik"]);
        assert_eq!(d.history.lock().unwrap().items[0].final_text.as_deref(), Some("ask Karthik now"));

        p.set_fields(vec![field("", 2)]);
        process(&d, speech()).await;
        assert_eq!(p.pastes()[1], "ask Karthik now");
    }

    #[tokio::test]
    async fn all_zero_audio_reports_blocked_microphone() {
        let p = Arc::new(FakePlatform::new(vec![field("", 1)]));
        let d = deps(p.clone(), "anything", Arc::new(Echo));
        match process(&d, vec![0.0; 16_000]).await {
            Outcome::Failed { message } => assert!(message.contains("microphone"), "{message}"),
            other => panic!("expected Failed, got {other:?}"),
        }
        // Quiet but real input is still just silence.
        assert_eq!(process(&d, vec![0.0001; 16_000]).await, Outcome::Empty);
    }

    #[tokio::test]
    async fn password_field_blocks_and_keeps_history() {
        let p = Arc::new(FakePlatform::new(vec![Some(FieldText { value: "".into(), is_password: true, element_id: 1 })]));
        let d = deps(p.clone(), "my secret note", Arc::new(Echo));
        assert_eq!(process(&d, speech()).await, Outcome::Blocked { text: "my secret note".into() });
        assert_eq!(d.history.lock().unwrap().items[0].cleaned, "my secret note");
        assert!(matches!(p.snapshot_clipboard(), ClipboardSnapshot::Text(t) if t == "my secret note"));
        assert!(p.pastes().is_empty());
    }

    #[tokio::test]
    async fn provider_down_inserts_raw_with_swaps() {
        let p = Arc::new(FakePlatform::new(vec![field("", 1)]));
        let d = deps(p.clone(), "tell kartik", Arc::new(Down));
        d.dict.lock().unwrap().upsert_manual("Karthik", vec!["kartik".into()], None, 1);
        let o = process(&d, speech()).await;
        assert!(matches!(&o, Outcome::Inserted { text, skipped: Some(Skip::EngineError), .. } if text == "tell Karthik"));
    }

    /// The bundled engine's only step: answers "bundled", or fails to start.
    struct FakeBundled {
        ok: bool,
        starts: Arc<AtomicUsize>,
    }
    struct Says(&'static str);
    #[async_trait]
    impl CleanupProvider for Says {
        fn base_timeout(&self) -> Duration {
            Duration::from_millis(200)
        }
        async fn clean(&self, _s: &str, _t: &str) -> anyhow::Result<String> {
            Ok(self.0.into())
        }
    }
    #[async_trait]
    impl Candidate for FakeBundled {
        fn name(&self) -> &'static str {
            "fake"
        }
        async fn start(&self) -> anyhow::Result<Running> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            anyhow::ensure!(self.ok, "model missing");
            Ok(Running { provider: Arc::new(Says("bundled")), child: None })
        }
    }

    fn switch(kind: LocalEngineKind, ok: bool) -> (Arc<SwitchProvider>, Arc<AtomicUsize>) {
        let starts = Arc::new(AtomicUsize::new(0));
        let local = LocalEngine::new(vec![Box::new(FakeBundled { ok, starts: starts.clone() })]);
        let settings = Arc::new(Mutex::new(Settings { local_engine: kind, ..Default::default() }));
        (Arc::new(SwitchProvider { settings, local: Arc::new(Mutex::new(local)) }), starts)
    }

    #[tokio::test]
    async fn switch_provider_uses_local_engine_when_bundled() {
        let (sp, starts) = switch(LocalEngineKind::Bundled, true);
        let d = deps(Arc::new(FakePlatform::new(vec![field("", 1)])), "hello there", sp.clone());
        assert_eq!(inserted(&process(&d, speech()).await).2, "bundled");
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        assert!(!sp.paused());
    }

    #[tokio::test]
    async fn switch_provider_uses_ollama_for_upgraders() {
        let (sp, starts) = switch(LocalEngineKind::Ollama, true);
        assert_eq!(sp.base_timeout(), OllamaProvider::default().base_timeout());
        // Ollama may or may not run on this machine; either way the bundled engine never answers.
        let r = tokio::time::timeout(Duration::from_millis(500), sp.clean("s", "hello there")).await;
        assert!(!matches!(r, Ok(Ok(ref t)) if t == "bundled"));
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert!(!sp.paused());
    }

    #[tokio::test]
    async fn switch_provider_reports_paused_when_no_engine_starts() {
        let (sp, _) = switch(LocalEngineKind::Bundled, false);
        let d = deps(Arc::new(FakePlatform::new(vec![field("", 1)])), "hello there", sp.clone());
        assert!(matches!(process(&d, speech()).await, Outcome::Inserted { skipped: Some(_), .. }));
        assert!(sp.paused());
    }

    #[tokio::test]
    async fn no_readable_field_learns_nothing() {
        let p = Arc::new(FakePlatform::new(vec![field("", 1), None]));
        let mut d = deps(p.clone(), "ask kartik now", Arc::new(Echo));
        d.poll = Duration::from_millis(250);
        d.max_observe = Duration::from_secs(60);
        let (at, id, text) = inserted(&process(&d, speech()).await);
        let (_tx, rx) = std::sync::mpsc::channel();
        let t = Instant::now();
        assert!(learn_after(&d, text, at, id, Arc::new(AtomicBool::new(false)), rx).await.is_empty());
        assert!(t.elapsed() < Duration::from_secs(1));
        assert!(d.dict.lock().unwrap().entries.is_empty());
    }

    #[tokio::test]
    async fn next_dictation_ends_previous_observation() {
        let p = Arc::new(FakePlatform::new(vec![field("", 1), field("ask kartik now", 1), field("ask kartik now", 1), field("ask Karthik now", 1)]));
        let d = deps(p.clone(), "ask kartik now", Arc::new(Echo));
        let (at, id, text) = inserted(&process(&d, speech()).await);
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            c2.store(true, Ordering::SeqCst);
        });
        let (_tx, rx) = std::sync::mpsc::channel();
        let t = Instant::now();
        let events = learn_after(&d, text, at, id, cancel, rx).await;
        assert!(t.elapsed() < Duration::from_secs(1));
        assert_eq!(events.len(), 1);
        assert!(d.dict.lock().unwrap().get("Karthik").is_some());
    }

    #[tokio::test]
    async fn learning_off_learns_nothing() {
        let p = Arc::new(FakePlatform::new(vec![field("", 1), field("ask kartik now", 1), field("ask kartik now", 1), field("ask Karthik now", 1)]));
        let d = deps(p.clone(), "ask kartik now", Arc::new(Echo));
        d.settings.lock().unwrap().learn_from_corrections = false;
        let (at, id, text) = inserted(&process(&d, speech()).await);
        assert!(learn_after(&d, text, at, id, Arc::new(AtomicBool::new(false)), enter_now()).await.is_empty());
        assert!(d.dict.lock().unwrap().entries.is_empty());
    }

    #[tokio::test]
    async fn history_off_stores_nothing() {
        let p = Arc::new(FakePlatform::new(vec![field("", 1), field("ask kartik now", 1), field("ask kartik now", 1), field("ask Karthik now", 1)]));
        let d = deps(p.clone(), "ask kartik now", Arc::new(Echo));
        d.settings.lock().unwrap().keep_history = false;
        let (at, id, text) = inserted(&process(&d, speech()).await);
        learn_after(&d, text, at, id, Arc::new(AtomicBool::new(false)), enter_now()).await;
        assert!(d.history.lock().unwrap().items.is_empty());
    }
}
