//! Speech recognition with whisper.cpp. Takes 16 kHz mono samples; no OS calls.
use crate::settings::Language;
use anyhow::Context;
use std::path::Path;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub const WHISPER_MODELS: [(&str, &str); 2] = [
    (
        "ggml-large-v3-turbo-q8_0.bin",
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q8_0.bin",
    ),
    ("ggml-small-q8_0.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q8_0.bin"),
];

pub trait Transcriber: Send + Sync {
    fn transcribe(&self, samples_16k: &[f32], lang: Language) -> anyhow::Result<String>;
}

pub struct WhisperAsr {
    ctx: WhisperContext,
}

impl WhisperAsr {
    pub fn load(model_path: &Path) -> anyhow::Result<WhisperAsr> {
        let path = model_path.to_str().context("model path is not UTF-8")?;
        let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default())
            .with_context(|| format!("loading Whisper model {path}"))?;
        Ok(WhisperAsr { ctx })
    }
}

fn lang_code(lang: Language) -> &'static str {
    match lang {
        Language::English => "en",
        Language::Hindi | Language::Hinglish => "hi",
        Language::Auto => "auto",
    }
}

impl Transcriber for WhisperAsr {
    fn transcribe(&self, samples_16k: &[f32], lang: Language) -> anyhow::Result<String> {
        let mut state = self.ctx.create_state()?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(lang_code(lang)));
        params.set_no_timestamps(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        params.set_n_threads(threads as i32);
        state.full(params, samples_16k)?;
        let text: Vec<String> = state.as_iter().filter_map(|s| s.to_str_lossy().ok().map(|t| t.trim().to_string())).collect();
        Ok(text.join(" ").trim().to_string())
    }
}
