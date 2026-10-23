//! The engines behind a dictation, chosen from settings at call time: the
//! cleanup provider (Local, Gemini, Haiku) and the Whisper model (loaded lazily).
use crate::commands::api_key;
use crate::local_engine::{EngineStatus, LocalEngine};
use async_trait::async_trait;
use shabdly_core::asr::{Transcriber, WhisperAsr};
use shabdly_core::providers::{CleanupProvider, GeminiProvider, HaikuProvider, OllamaProvider, Skip};
use shabdly_core::settings::{Language, LocalEngineKind, ProviderKind, Settings};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The bundled engine in use; the Speed switch swaps in a new one once it has started.
pub type EngineSlot = Arc<Mutex<Arc<LocalEngine>>>;

pub struct SwitchProvider {
    pub settings: Arc<Mutex<Settings>>,
    pub local: EngineSlot,
}

impl SwitchProvider {
    fn local(&self) -> Arc<LocalEngine> {
        self.local.lock().unwrap().clone()
    }

    fn kind(&self) -> ProviderKind {
        self.settings.lock().unwrap().provider
    }

    /// Local served by the bundled engine (new installs), not by Ollama (0.1.5 upgraders).
    fn bundled(&self) -> bool {
        let s = self.settings.lock().unwrap();
        s.provider == ProviderKind::Local && s.local_engine == LocalEngineKind::Bundled
    }
}

#[async_trait]
impl CleanupProvider for SwitchProvider {
    fn base_timeout(&self) -> Duration {
        match self.kind() {
            ProviderKind::Local if self.bundled() => self.local().base_timeout(),
            ProviderKind::Local => OllamaProvider::default().base_timeout(),
            _ => Duration::from_secs(30),
        }
    }

    async fn clean(&self, system: &str, text: &str) -> anyhow::Result<String> {
        match self.kind() {
            ProviderKind::Local if self.bundled() => {
                // Switched to the bundled engine after launch: start it now.
                let local = self.local();
                if local.status() == EngineStatus::Starting {
                    local.ensure().await;
                }
                local.clean(system, text).await
            }
            ProviderKind::Local => OllamaProvider::default().clean(system, text).await,
            ProviderKind::GeminiFree | ProviderKind::GeminiPaid => {
                let key = api_key("gemini").ok_or(Skip::NoGeminiKey)?;
                GeminiProvider::new(key).clean(system, text).await
            }
            ProviderKind::Haiku => {
                let key = api_key("anthropic").ok_or(Skip::NoClaudeKey)?;
                HaikuProvider { key }.clean(system, text).await
            }
        }
    }

    fn paused(&self) -> bool {
        self.bundled() && matches!(self.local().status(), EngineStatus::Paused { .. })
    }
}

/// Whisper, loaded on first use and reloaded when the model setting changes.
pub struct LazyAsr {
    pub models_dir: PathBuf,
    pub settings: Arc<Mutex<Settings>>,
    loaded: Mutex<Option<(String, WhisperAsr)>>,
}

impl LazyAsr {
    pub fn new(models_dir: PathBuf, settings: Arc<Mutex<Settings>>) -> Self {
        LazyAsr { models_dir, settings, loaded: Mutex::new(None) }
    }

    fn with_model<T>(&self, f: impl FnOnce(&WhisperAsr) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let name = self.settings.lock().unwrap().whisper_model.clone();
        let mut slot = self.loaded.lock().unwrap();
        if slot.as_ref().is_none_or(|(n, _)| *n != name) {
            let path = self.models_dir.join(&name);
            if !path.exists() {
                anyhow::bail!("Speech model not downloaded yet. Open Shabdly to finish setup.");
            }
            *slot = Some((name, WhisperAsr::load(&path)?));
        }
        f(&slot.as_ref().unwrap().1)
    }

    /// Load the model ahead of the first dictation.
    pub fn preload(&self) {
        let _ = self.with_model(|_| Ok(()));
    }

    /// Free the model. Its Metal buffers must be gone before exit, or ggml's static
    /// Metal device destructor aborts the process (GGML_ASSERT on residency sets).
    pub fn unload(&self) {
        self.loaded.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

impl Transcriber for LazyAsr {
    fn transcribe(&self, samples_16k: &[f32], lang: Language) -> anyhow::Result<String> {
        self.with_model(|asr| asr.transcribe(samples_16k, lang))
    }
}
