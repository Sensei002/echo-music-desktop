//! The 10-band parametric equalizer and stereo widener.
//!
//! Upstream ships a custom `BiquadFilter`-based equalizer. This is the same
//! design expressed as a `rodio::Source` adapter: the filter chain sits between
//! the decoder and the output device, and the gains live behind a shared handle
//! so moving a slider takes effect immediately — no need to restart playback.

use parking_lot::Mutex;
use rodio::Source;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Centre frequencies of the 10 ISO bands, in Hz.
pub const BANDS: [f32; 10] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];

/// Filter resonance — one octave of bandwidth per band.
const Q: f32 = 1.41;

/// A single peaking-EQ biquad section (Direct Form I).
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }
}

impl Biquad {
    /// Builds a peaking filter for `freq` Hz with `gain_db` of boost/cut.
    pub fn peaking(sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        if sample_rate <= 0.0 || freq <= 0.0 || freq >= sample_rate / 2.0 || gain_db.abs() < 1e-4 {
            return Self::default();
        }
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / sample_rate;
        let cos_w0 = w0.cos();
        let alpha = w0.sin() / (2.0 * q);

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            ..Default::default()
        }
    }

    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        let output = self.b0 * input + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;
        output
    }

    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// The live equalizer configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct EqConfig {
    pub enabled: bool,
    /// Gain in dB per band, same order as [`BANDS`].
    pub gains: Vec<f32>,
    /// 0.0 = unchanged stereo image; 1.0 = fully widened.
    pub stereo_width: f32,
    /// Linear pre-amplification applied after the filters.
    pub preamp: f32,
}

impl Default for EqConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            gains: vec![0.0; BANDS.len()],
            stereo_width: 0.0,
            preamp: 1.0,
        }
    }
}

impl EqConfig {
    /// The "flat" preset — every band at 0 dB.
    pub fn flat() -> Self {
        Self::default()
    }

    /// Named presets matching the upstream equalizer screen.
    pub fn preset(name: &str) -> Self {
        let gains: Vec<f32> = match name {
            "bass_boost" | "Bass boost" => vec![6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "treble_boost" | "Treble boost" => vec![0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 3.0, 4.0, 5.0, 6.0],
            "vocal" | "Vocal" => vec![-2.0, -1.0, 0.0, 2.0, 4.0, 4.0, 3.0, 1.0, 0.0, -1.0],
            "loudness" | "Loudness" => vec![5.0, 4.0, 1.0, 0.0, -1.0, 0.0, 1.0, 3.0, 4.0, 5.0],
            "rock" | "Rock" => vec![4.0, 3.0, 1.0, -1.0, -2.0, -1.0, 1.0, 3.0, 4.0, 4.0],
            "pop" | "Pop" => vec![-1.0, 0.0, 2.0, 3.0, 3.0, 1.0, 0.0, 0.0, 1.0, 2.0],
            "acoustic" | "Acoustic" => vec![3.0, 2.0, 1.0, 0.0, 1.0, 2.0, 2.0, 3.0, 3.0, 2.0],
            _ => vec![0.0; BANDS.len()],
        };
        Self {
            enabled: true,
            gains,
            stereo_width: 0.0,
            preamp: 1.0,
        }
    }

    /// Every preset name offered by the UI.
    pub fn presets() -> &'static [&'static str] {
        &[
            "flat",
            "bass_boost",
            "treble_boost",
            "vocal",
            "loudness",
            "rock",
            "pop",
            "acoustic",
        ]
    }
}

/// A cheap, cloneable handle to the live equalizer configuration.
#[derive(Clone)]
pub struct EqHandle {
    config: Arc<Mutex<EqConfig>>,
    version: Arc<AtomicU64>,
}

impl Default for EqHandle {
    fn default() -> Self {
        Self::new(EqConfig::default())
    }
}

impl EqHandle {
    pub fn new(config: EqConfig) -> Self {
        Self {
            config: Arc::new(Mutex::new(config)),
            version: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Replaces the configuration, signalling every attached source.
    pub fn update(&self, config: EqConfig) {
        *self.config.lock() = config;
        self.version.fetch_add(1, Ordering::Release);
    }

    pub fn snapshot(&self) -> EqConfig {
        self.config.lock().clone()
    }

    fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }
}

/// Wraps a source with the equalizer and stereo widener.
pub struct EqualizerSource<S: Source> {
    inner: S,
    handle: EqHandle,
    seen_version: u64,
    filters: Vec<Biquad>,
    channels: u16,
    pending: VecDeque<f32>,
}

impl<S: Source> EqualizerSource<S> {
    pub fn new(inner: S, handle: EqHandle) -> Self {
        let channels = inner.channels();
        let sample_rate = inner.sample_rate() as f32;
        let config = handle.snapshot();
        let filters = build_filters(sample_rate, &config);
        Self {
            inner,
            seen_version: handle.version(),
            handle,
            filters,
            channels,
            pending: VecDeque::with_capacity(channels as usize),
        }
    }

    /// Rebuilds the filter chain when the configuration changed.
    fn refresh(&mut self) {
        let version = self.handle.version();
        if version == self.seen_version {
            return;
        }
        self.seen_version = version;
        let config = self.handle.snapshot();
        let sample_rate = self.inner.sample_rate() as f32;
        self.filters = build_filters(sample_rate, &config);
    }
}

fn build_filters(sample_rate: f32, config: &EqConfig) -> Vec<Biquad> {
    if !config.enabled {
        return Vec::new();
    }
    BANDS
        .iter()
        .enumerate()
        .map(|(index, freq)| {
            let gain = config.gains.get(index).copied().unwrap_or(0.0);
            Biquad::peaking(sample_rate, *freq, Q, gain)
        })
        .collect()
}

impl<S: Source> Iterator for EqualizerSource<S> {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if let Some(value) = self.pending.pop_front() {
            return Some(value);
        }
        self.refresh();

        let config = self.handle.snapshot();
        let channels = self.channels.max(1) as usize;

        let mut frame = Vec::with_capacity(channels);
        for _ in 0..channels {
            match self.inner.next() {
                Some(sample) => frame.push(sample),
                None => break,
            }
        }
        if frame.is_empty() {
            return None;
        }

        if !self.filters.is_empty() {
            for sample in frame.iter_mut() {
                let mut value = *sample;
                for filter in self.filters.iter_mut() {
                    value = filter.process(value);
                }
                *sample = value;
            }
        }

        if config.preamp != 1.0 {
            for sample in frame.iter_mut() {
                *sample *= config.preamp;
            }
        }

        // Mid/side stereo widening (only meaningful for stereo material).
        if channels >= 2 && config.stereo_width > 0.0 {
            for pair in frame.chunks_mut(2) {
                if let [left, right] = pair {
                    let mid = (*left + *right) * 0.5;
                    let side = (*left - *right) * 0.5 * (1.0 + config.stereo_width);
                    *left = mid + side;
                    *right = mid - side;
                }
            }
        }

        self.pending.extend(frame);
        self.pending.pop_front()
    }
}

impl<S: Source> Source for EqualizerSource<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_eq_is_transparent() {
        let mut filter = Biquad::peaking(44_100.0, 1_000.0, Q, 0.0);
        for input in [0.0f32, 0.5, -0.5, 1.0] {
            assert!((filter.process(input) - input).abs() < 1e-6);
        }
    }

    #[test]
    fn boost_raises_energy_at_the_centre_frequency() {
        let sample_rate = 48_000.0;
        let freq = 1_000.0;
        let mut boosted = Biquad::peaking(sample_rate, freq, Q, 12.0);
        let mut flat = Biquad::peaking(sample_rate, freq, Q, 0.0);

        let mut boosted_energy = 0.0f32;
        let mut flat_energy = 0.0f32;
        for n in 0..4_800 {
            let t = n as f32 / sample_rate;
            let input = (2.0 * std::f32::consts::PI * freq * t).sin();
            // Skip the transient.
            let b = boosted.process(input);
            let f = flat.process(input);
            if n > 1_000 {
                boosted_energy += b * b;
                flat_energy += f * f;
            }
        }
        assert!(boosted_energy > flat_energy * 2.0);
    }

    #[test]
    fn presets_have_the_right_shape() {
        let bass = EqConfig::preset("bass_boost");
        assert_eq!(bass.gains.len(), BANDS.len());
        assert!(bass.gains[0] > bass.gains[9]);
        assert!(bass.enabled);

        let flat = EqConfig::preset("nope");
        assert!(flat.gains.iter().all(|g| *g == 0.0));
    }

    #[test]
    fn handle_signals_version_changes() {
        let handle = EqHandle::default();
        let before = handle.version();
        handle.update(EqConfig::preset("rock"));
        assert_ne!(handle.version(), before);
        assert_eq!(handle.snapshot().gains.len(), BANDS.len());
    }
}
