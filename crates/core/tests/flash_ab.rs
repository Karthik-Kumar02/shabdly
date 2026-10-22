//! Flash attention on vs off: speed and character error rate on real recordings.
//! SHABDLY_MODEL=<ggml model> BENCH_DIR=<folder with refs.tsv and <id>.wav>
//! refs.tsv lines: id<TAB>voice-or-note<TAB>reference text
use shabdly_core::asr::{Transcriber, WhisperAsr, context_params};
use shabdly_core::audio::to_16k_mono;
use shabdly_core::settings::Language;
use std::path::Path;
use std::time::Instant;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperState, get_lang_str};

fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// Flash attention off, same Auto flow as `WhisperAsr`: detect state primed with audio_ctx 500.
struct NoFlash {
    ctx: WhisperContext,
    detect: WhisperState,
}

impl NoFlash {
    fn load(path: &str) -> NoFlash {
        let ctx = WhisperContext::new_with_params(path, context_params(false)).unwrap();
        let mut detect = ctx.create_state().unwrap();
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en"));
        params.set_audio_ctx(500);
        params.set_max_tokens(1);
        params.set_print_progress(false);
        detect.full(params, &[0.0; 16_000]).unwrap();
        NoFlash { ctx, detect }
    }

    fn transcribe(&mut self, pcm: &[f32]) -> String {
        let lang = self
            .detect
            .pcm_to_mel(pcm, threads())
            .and_then(|_| self.detect.lang_detect(0, threads()))
            .ok()
            .and_then(|(id, _)| get_lang_str(id))
            .unwrap_or("auto");
        let mut state = self.ctx.create_state().unwrap();
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(lang));
        params.set_no_timestamps(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_n_threads(threads() as i32);
        state.full(params, pcm).unwrap();
        let text: Vec<String> = state.as_iter().filter_map(|s| s.to_str_lossy().ok().map(|t| t.trim().to_string())).collect();
        text.join(" ").trim().to_string()
    }
}

fn read_wav(path: &Path) -> Vec<f32> {
    let mut wav = hound::WavReader::open(path).unwrap();
    let spec = wav.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => wav.samples::<f32>().map(|s| s.unwrap()).collect(),
        hound::SampleFormat::Int => {
            let scale = (1u32 << (spec.bits_per_sample - 1)) as f32;
            wav.samples::<i32>().map(|s| s.unwrap() as f32 / scale).collect()
        }
    };
    to_16k_mono(&samples, spec.channels, spec.sample_rate)
}

/// Lowercase, keep alphanumerics and Devanagari (U+0900..U+097F) minus the danda marks.
fn normalize(s: &str) -> Vec<char> {
    s.to_lowercase()
        .chars()
        .filter(|&c| c.is_alphanumeric() || (('\u{0900}'..='\u{097F}').contains(&c) && c != '।' && c != '॥'))
        .collect()
}

fn cer(reference: &str, hyp: &str) -> f64 {
    let (r, h) = (normalize(reference), normalize(hyp));
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    for (i, rc) in r.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, hc) in h.iter().enumerate() {
            cur.push((prev[j] + usize::from(rc != hc)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[h.len()] as f64 / r.len().max(1) as f64
}

/// Warm once, then the median of 3 runs in ms, plus the last output.
fn median_ms(mut run: impl FnMut() -> String) -> (f64, String) {
    run();
    let mut out = String::new();
    let mut ms: Vec<f64> = (0..3)
        .map(|_| {
            let t = Instant::now();
            out = run();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    (ms[1], out)
}

#[test]
fn cer_ignores_case_punctuation_and_danda() {
    assert_eq!(cer("Hello, world.", "hello world"), 0.0);
    assert_eq!(cer("नमस्ते।", "नमस्ते"), 0.0);
    assert_eq!(cer("abcd", "abxd"), 0.25);
}

#[test]
#[ignore]
fn flash_ab() {
    let model = std::env::var("SHABDLY_MODEL").expect("set SHABDLY_MODEL");
    let dir = std::path::PathBuf::from(std::env::var("BENCH_DIR").expect("set BENCH_DIR"));
    let refs = std::fs::read_to_string(dir.join("refs.tsv")).unwrap();
    let mut off_asr = NoFlash::load(&model);
    let on_asr = WhisperAsr::load(Path::new(&model)).unwrap();
    let mut rows = Vec::new();
    for line in refs.lines().filter(|l| !l.trim().is_empty()) {
        let cols: Vec<&str> = line.splitn(3, '\t').collect();
        let (id, reference) = (cols[0], cols.get(2).copied().unwrap_or(""));
        let pcm = read_wav(&dir.join(format!("{id}.wav")));
        let (off_ms, off_text) = median_ms(|| off_asr.transcribe(&pcm));
        let (on_ms, on_text) = median_ms(|| on_asr.transcribe(&pcm, Language::Auto).unwrap());
        let row = [off_ms, on_ms, cer(reference, &off_text) * 100.0, cer(reference, &on_text) * 100.0];
        println!("AB\t{id}\t{:.0}\t{:.0}\t{:.2}\t{:.2}", row[0], row[1], row[2], row[3]);
        rows.push(row);
    }
    assert!(!rows.is_empty(), "no clips in refs.tsv");
    let mean = |k: usize| rows.iter().map(|r| r[k]).sum::<f64>() / rows.len() as f64;
    println!("MEAN\t{:.0}\t{:.0}\t{:.2}\t{:.2}", mean(0), mean(1), mean(2), mean(3));
}
