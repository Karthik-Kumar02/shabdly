//! Microphone capture with cpal. Lives on the controller thread (cpal streams
//! are not Send on every OS).
use anyhow::Context;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const LEVEL_EVERY: Duration = Duration::from_millis(50);

pub struct Recorder {
    stream: Option<cpal::Stream>,
    buf: Arc<Mutex<Vec<f32>>>,
    channels: u16,
    rate: u32,
}

impl Default for Recorder {
    fn default() -> Self {
        Recorder { stream: None, buf: Default::default(), channels: 1, rate: 16_000 }
    }
}

impl Recorder {
    /// Start recording from the default input; mic levels go to `level_tx` at 20 Hz.
    pub fn start(&mut self, level_tx: Sender<f32>) -> anyhow::Result<()> {
        self.stream = None;
        let device = cpal::default_host().default_input_device().context("no microphone found")?;
        let supported = device.default_input_config().context("microphone unavailable")?;
        self.channels = supported.channels();
        self.rate = supported.sample_rate();
        self.buf.lock().unwrap().clear();
        let buf = self.buf.clone();
        let mut chunk: Vec<f32> = Vec::new();
        let mut last = Instant::now();
        let mut push = move |samples: &mut dyn Iterator<Item = f32>| {
            let start = chunk.len();
            chunk.extend(samples);
            buf.lock().unwrap().extend_from_slice(&chunk[start..]);
            if last.elapsed() >= LEVEL_EVERY {
                let _ = level_tx.send(shabdly_core::audio::rms(&chunk));
                chunk.clear();
                last = Instant::now();
            }
        };
        let err = |e| eprintln!("microphone stream error: {e}");
        let config = supported.config();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => {
                device.build_input_stream::<f32, _, _>(config, move |d, _| push(&mut d.iter().copied()), err, None)
            }
            cpal::SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
                config,
                move |d, _| push(&mut d.iter().map(|s| *s as f32 / 32768.0)),
                err,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream::<u16, _, _>(
                config,
                move |d, _| push(&mut d.iter().map(|s| (*s as f32 - 32768.0) / 32768.0)),
                err,
                None,
            ),
            other => anyhow::bail!("unsupported microphone format {other:?}"),
        }
        .context("could not open the microphone")?;
        stream.play().context("could not start the microphone")?;
        self.stream = Some(stream);
        Ok(())
    }

    /// Stop and return (samples, channels, rate).
    pub fn stop(&mut self) -> (Vec<f32>, u16, u32) {
        self.stream = None;
        (std::mem::take(&mut *self.buf.lock().unwrap()), self.channels, self.rate)
    }
}
