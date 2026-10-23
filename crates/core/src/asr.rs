//! Speech recognition with whisper.cpp. Takes 16 kHz mono samples; no OS calls.
use crate::models::ModelFile;
use crate::settings::Language;
use anyhow::Context;
use std::path::Path;
use std::sync::Mutex;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState, get_lang_str};

/// Speech models, pinned to a commit and downloaded like the engine models (resume, retry, sha256).
pub const WHISPER_MODELS: [ModelFile; 2] = [
    ModelFile {
        name: "ggml-large-v3-turbo-q8_0.bin",
        size: 874_188_075,
        sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-q8_0.bin",
    },
    ModelFile {
        name: "ggml-small-q8_0.bin",
        size: 264_464_607,
        sha256: "49c8fb02b65e6049d5fa6c04f81f53b867b5ec9540406812c643f177317f779f",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-small-q8_0.bin",
    },
];

pub trait Transcriber: Send + Sync {
    fn transcribe(&self, samples_16k: &[f32], lang: Language) -> anyhow::Result<String>;
}

/// Encoder frames used to detect the language: 10 s instead of 30 s. Matched
/// full-window detection on English, Hindi and Hinglish clips at a third of the cost.
const DETECT_AUDIO_CTX: i32 = 500;

/// Whisper context options. `flash` turns on whisper.cpp flash attention.
pub fn context_params(flash: bool) -> WhisperContextParameters<'static> {
    let mut p = WhisperContextParameters::default();
    p.flash_attn(flash);
    p
}

pub struct WhisperAsr {
    ctx: WhisperContext,
    /// Language detection only. whisper.cpp keeps the last `audio_ctx` a state ran
    /// with and detection reuses it, so this state is primed once with a short one.
    detect: Mutex<WhisperState>,
}

impl WhisperAsr {
    pub fn load(model_path: &Path) -> anyhow::Result<WhisperAsr> {
        let path = model_path.to_str().context("model path is not UTF-8")?;
        // Flash attention: about 0.15 to 0.2 s faster per dictation on Apple Silicon.
        let ctx = match WhisperContext::new_with_params(path, context_params(true)) {
            Ok(ctx) => ctx,
            Err(e) => {
                eprintln!("flash attention unavailable, loading without it: {e}");
                WhisperContext::new_with_params(path, context_params(false))
                    .with_context(|| format!("loading Whisper model {path}"))?
            }
        };
        let mut detect = ctx.create_state()?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_audio_ctx(DETECT_AUDIO_CTX);
        params.set_max_tokens(1);
        params.set_print_progress(false);
        detect.full(params, &[0.0; 16_000])?;
        Ok(WhisperAsr { ctx, detect: Mutex::new(detect) })
    }

    /// Auto mode would otherwise run the full 30 s encoder twice: once to detect, once to transcribe.
    fn lang_code(&self, samples_16k: &[f32], lang: Language) -> &'static str {
        match lang {
            Language::English => "en",
            Language::Hindi | Language::Hinglish => "hi",
            Language::Auto => {
                let mut st = self.detect.lock().unwrap();
                st.pcm_to_mel(samples_16k, threads())
                    .and_then(|_| st.lang_detect(0, threads()))
                    .ok()
                    .and_then(|(id, _)| get_lang_str(id))
                    .unwrap_or("auto")
            }
        }
    }
}

fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

impl Transcriber for WhisperAsr {
    fn transcribe(&self, samples_16k: &[f32], lang: Language) -> anyhow::Result<String> {
        let mut state = self.ctx.create_state()?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(self.lang_code(samples_16k, lang)));
        params.set_no_timestamps(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_n_threads(threads() as i32);
        state.full(params, samples_16k)?;
        let text: Vec<String> = state.as_iter().filter_map(|s| s.to_str_lossy().ok().map(|t| t.trim().to_string())).collect();
        Ok(text.join(" ").trim().to_string())
    }
}
