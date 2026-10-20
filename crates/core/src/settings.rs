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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub version: u32,
    pub language: Language,
    pub provider: ProviderKind,
    pub hotkey: String,
    pub learn_from_corrections: bool,
    pub keep_history: bool,
    pub whisper_model: String,
}

pub fn default_hotkey() -> &'static str {
    if cfg!(target_os = "windows") { "Ctrl+Win" } else { "Fn" }
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
        }
    }
}
