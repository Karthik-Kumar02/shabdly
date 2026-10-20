//! Audio helpers: whatever the mic gives us becomes 16 kHz mono for Whisper.

const TARGET_RATE: u32 = 16_000;

/// Average interleaved channels, then resample to 16 kHz.
// ponytail: linear resample, fine for speech; use rubato if accuracy suffers
pub fn to_16k_mono(samples: &[f32], channels: u16, rate: u32) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    let mono: Vec<f32> = samples.chunks_exact(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect();
    if rate == TARGET_RATE || mono.is_empty() {
        return mono;
    }
    let out_len = (mono.len() as u64 * TARGET_RATE as u64 / rate as u64) as usize;
    let step = rate as f64 / TARGET_RATE as f64;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * step;
            let j = pos as usize;
            let frac = (pos - j as f64) as f32;
            let a = mono[j];
            let b = *mono.get(j + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}
