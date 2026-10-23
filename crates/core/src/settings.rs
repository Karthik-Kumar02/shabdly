use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    English,
    Hindi,
    Hinglish,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Local,
    GeminiFree,
    GeminiPaid,
    Haiku,
}

/// Which engine serves `provider: local`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalEngineKind {
    Bundled,
    Ollama,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speed {
    Best,
    Fast,
}

/// New fields carry serde defaults so 0.1.5 files load, and 0.1.5 (which ignores unknown
/// fields) still reads files written by this version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub version: u32,
    pub language: Language,
    pub provider: ProviderKind,
    pub hotkey: String,
    pub learn_from_corrections: bool,
    pub keep_history: bool,
    pub whisper_model: String,
    /// Missing means a 0.1.5 upgrader, who keeps Ollama.
    #[serde(default = "upgrade_engine")]
    pub local_engine: LocalEngineKind,
    /// `None` means the catalog default for this machine.
    #[serde(default)]
    pub speed: Option<Speed>,
    #[serde(default)]
    pub builtin_offer_done: bool,
}

fn upgrade_engine() -> LocalEngineKind {
    LocalEngineKind::Ollama
}

pub fn default_hotkey() -> &'static str {
    if cfg!(target_os = "windows") { "Ctrl+Win" } else { "Fn" }
}

impl Settings {
    /// Local served by the bundled engine (new installs), not by Ollama (0.1.5 upgraders).
    pub fn bundled(&self) -> bool {
        self.provider == ProviderKind::Local && self.local_engine == LocalEngineKind::Bundled
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: 1,
            language: Language::Auto,
            provider: ProviderKind::Local,
            hotkey: default_hotkey().to_string(),
            learn_from_corrections: true,
            keep_history: true,
            whisper_model: "ggml-large-v3-turbo-q8_0.bin".to_string(),
            local_engine: LocalEngineKind::Bundled,
            speed: None,
            builtin_offer_done: true,
        }
    }
}
