use shabdly_core::audio::{rms, to_16k_mono};

#[test]
fn stereo_48k_to_16k_mono_length() {
    let stereo = vec![0.5f32; 48_000 * 2];
    let out = to_16k_mono(&stereo, 2, 48_000);
    assert_eq!(out.len(), 16_000);
    assert!(out.iter().all(|s| (s - 0.5).abs() < 1e-6));
}

#[test]
fn mono_16k_passthrough() {
    let s: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
    assert_eq!(to_16k_mono(&s, 1, 16_000), s);
}

#[test]
fn channels_averaged() {
    assert_eq!(to_16k_mono(&[1.0, -1.0, 1.0, -1.0], 2, 16_000), vec![0.0, 0.0]);
}

#[test]
fn rms_of_silence_is_zero() {
    assert_eq!(rms(&[0.0; 320]), 0.0);
    assert_eq!(rms(&[]), 0.0);
    assert!((rms(&[0.5, -0.5]) - 0.5).abs() < 1e-6);
}

#[test]
fn whisper_prompt_is_mixed_script_except_for_english() {
    use shabdly_core::asr::initial_prompt;
    use shabdly_core::settings::Language;
    assert_eq!(initial_prompt(Language::English), None);
    for lang in [Language::Auto, Language::Hinglish, Language::Hindi] {
        let p = initial_prompt(lang).unwrap();
        assert!(p.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)), "{lang:?}: no Devanagari");
        assert!(p.split_whitespace().any(|w| w.chars().all(|c| c.is_ascii_alphabetic())), "{lang:?}: no Latin word");
    }
}

/// Needs the Whisper model: SHABDLY_MODEL=~/Downloads/ggml-large-v3-turbo-q8_0.bin
#[test]
#[ignore]
fn whisper_transcribes_fixture() {
    use shabdly_core::asr::{Transcriber, WhisperAsr};
    use shabdly_core::settings::Language;
    let model = std::env::var("SHABDLY_MODEL").expect("set SHABDLY_MODEL");
    let asr = WhisperAsr::load(std::path::Path::new(&model)).unwrap();
    let mut wav = hound::WavReader::open(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hello.wav")).unwrap();
    let spec = wav.spec();
    let samples: Vec<f32> = wav.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
    let pcm = to_16k_mono(&samples, spec.channels, spec.sample_rate);
    let text = asr.transcribe(&pcm, Language::Auto).unwrap();
    assert!(text.to_lowercase().contains("hello"), "got: {text}");
}
