use super::AudioError;

pub const SAMPLE_RATE: u32 = 16000;
pub const CHANNELS: u16 = 1;
/// RMS threshold for silence detection — low enough for cheap/quiet microphones
pub const SILENCE_RMS_THRESHOLD: f64 = 50.0;
/// Minimum samples to consider a valid recording (~0.3s at 16kHz)
pub const MIN_RECORDING_SAMPLES: usize = 4800;
/// Emit waveform data every ~64ms (1024 samples at 16kHz)
pub const WAVEFORM_EMIT_SAMPLES: usize = 1024;
/// Number of amplitude bars the frontend expects
pub const WAVEFORM_BARS: usize = 24;

/// Convert multi-channel f32 samples to mono i16
pub fn to_mono_i16_f32(samples: &[f32], channels: usize) -> Vec<i16> {
    samples
        .chunks(channels)
        .map(|frame| {
            let mono: f32 = frame.iter().sum::<f32>() / channels as f32;
            (mono * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16
        })
        .collect()
}

/// Convert multi-channel i16 samples to mono i16
pub fn to_mono_i16(samples: &[i16], channels: usize) -> Vec<i16> {
    if channels == 1 {
        return samples.to_vec();
    }
    samples
        .chunks(channels)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|&s| s as i32).sum();
            (sum / channels as i32) as i16
        })
        .collect()
}

/// Resample from source_rate to target_rate.
/// Uses averaging for integer-ratio downsampling (e.g. 48kHz -> 16kHz),
/// linear interpolation as fallback for non-integer ratios.
pub fn resample(samples: &[i16], source_rate: u32, target_rate: u32) -> Vec<i16> {
    if source_rate == target_rate || samples.is_empty() {
        return samples.to_vec();
    }

    // Integer-ratio downsampling with averaging (e.g. 48000/16000 = 3)
    if source_rate > target_rate && source_rate % target_rate == 0 {
        let ratio = (source_rate / target_rate) as usize;
        return samples
            .chunks(ratio)
            .map(|chunk| {
                let sum: i32 = chunk.iter().map(|&s| s as i32).sum();
                (sum / chunk.len() as i32) as i16
            })
            .collect();
    }

    // Fallback: linear interpolation for non-integer ratios
    let ratio = source_rate as f64 / target_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;

        let sample = if idx + 1 < samples.len() {
            let s0 = samples[idx] as f64;
            let s1 = samples[idx + 1] as f64;
            (s0 + frac * (s1 - s0)) as i16
        } else if idx < samples.len() {
            samples[idx]
        } else {
            0
        };

        output.push(sample);
    }

    output
}

/// Window size for speech detection (~50ms at 16kHz)
const SPEECH_WINDOW_SAMPLES: usize = 800;

pub fn has_speech(samples: &[i16]) -> (bool, f64) {
    if samples.is_empty() {
        return (false, 0.0);
    }

    let mut max_rms: f64 = 0.0;

    for window in samples.chunks(SPEECH_WINDOW_SAMPLES) {
        let sum_sq: f64 = window.iter().map(|&s| (s as f64) * (s as f64)).sum();
        let rms = (sum_sq / window.len() as f64).sqrt();
        if rms > max_rms {
            max_rms = rms;
        }
    }

    (max_rms >= SILENCE_RMS_THRESHOLD, max_rms)
}

pub fn encode_wav(samples: &[i16]) -> Result<Vec<u8>, AudioError> {
    let mut cursor = std::io::Cursor::new(Vec::new());

    let spec = hound::WavSpec {
        channels: CHANNELS,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::new(&mut cursor, spec)
        .map_err(|e| AudioError::EncodingError(e.to_string()))?;

    for &sample in samples {
        writer
            .write_sample(sample)
            .map_err(|e| AudioError::EncodingError(e.to_string()))?;
    }

    writer
        .finalize()
        .map_err(|e| AudioError::EncodingError(e.to_string()))?;

    Ok(cursor.into_inner())
}

/// Turns a finished 16 kHz mono capture into WAV bytes, rejecting recordings that are
/// too short or silent with `EmptyRecording`.
pub fn finish(samples: &[i16]) -> Result<Vec<u8>, AudioError> {
    if samples.len() < MIN_RECORDING_SAMPLES {
        return Err(AudioError::EmptyRecording);
    }
    let (speech_detected, peak_rms) = has_speech(samples);
    log::debug!(
        "[audio] Speech detection — peak RMS: {:.1}, threshold: {:.1}, detected: {}",
        peak_rms, SILENCE_RMS_THRESHOLD, speech_detected
    );
    if !speech_detected {
        return Err(AudioError::EmptyRecording);
    }
    encode_wav(samples)
}

/// Accumulates capture packets into the bar frames the recording bar draws.
#[derive(Default)]
pub struct Waveform {
    levels: Vec<f32>,
    samples: usize,
}

impl Waveform {
    pub fn new() -> Self {
        Self::default()
    }

    /// `mono` is one packet at the device rate and gives one level; `produced` is the
    /// number of 16 kHz samples it became, which paces the frames. Returns the last
    /// `WAVEFORM_BARS` levels each time another `WAVEFORM_EMIT_SAMPLES` have been produced.
    pub fn push(&mut self, mono: &[i16], produced: usize) -> Option<Vec<f32>> {
        self.levels.push(level(mono));
        let previous = self.samples;
        self.samples += produced;
        if previous / WAVEFORM_EMIT_SAMPLES == self.samples / WAVEFORM_EMIT_SAMPLES {
            return None;
        }
        let start = self.levels.len().saturating_sub(WAVEFORM_BARS);
        let bars = self.levels[start..].to_vec();
        self.levels.clear();
        Some(bars)
    }
}

/// Square root of the RMS, doubled, so quiet speech still moves the bars.
fn level(samples: &[i16]) -> f32 {
    let rms = if !samples.is_empty() {
        let sum: f32 = samples
            .iter()
            .map(|&s| {
                let f = s as f32 / i16::MAX as f32;
                f * f
            })
            .sum();
        (sum / samples.len() as f32).sqrt()
    } else {
        0.0
    };
    (rms.sqrt() * 2.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(len: usize, amplitude: f64) -> Vec<i16> {
        (0..len)
            .map(|i| {
                let phase = i as f64 * std::f64::consts::TAU * 440.0 / SAMPLE_RATE as f64;
                (phase.sin() * amplitude * i16::MAX as f64) as i16
            })
            .collect()
    }

    #[test]
    fn finish_rejects_short_recordings() {
        let samples = sine(MIN_RECORDING_SAMPLES - 1, 0.5);
        assert!(matches!(finish(&samples), Err(AudioError::EmptyRecording)));
    }

    #[test]
    fn finish_rejects_silence() {
        let samples = vec![10i16; SAMPLE_RATE as usize];
        assert!(matches!(finish(&samples), Err(AudioError::EmptyRecording)));
    }

    #[test]
    fn finish_encodes_speech_as_16k_mono_wav() {
        let samples = sine(MIN_RECORDING_SAMPLES, 0.5);
        let wav = finish(&samples).unwrap();
        let reader = hound::WavReader::new(std::io::Cursor::new(wav)).unwrap();
        assert_eq!(reader.spec().sample_rate, SAMPLE_RATE);
        assert_eq!(reader.spec().channels, CHANNELS);
        assert_eq!(reader.len() as usize, MIN_RECORDING_SAMPLES);
    }

    #[test]
    fn waveform_emits_once_per_emit_window() {
        let mut waveform = Waveform::new();
        let packet = sine(160, 0.1);
        let mut frames = Vec::new();
        for i in 1..=20 {
            if let Some(bars) = waveform.push(&packet, packet.len()) {
                assert!(bars.len() <= WAVEFORM_BARS);
                frames.push(i);
            }
        }
        // 160-sample packets cross a 1024-sample boundary on packets 7, 13 and 20.
        assert_eq!(frames, vec![7, 13, 20]);
    }

    #[test]
    fn waveform_keeps_the_last_bars() {
        let mut waveform = Waveform::new();
        for _ in 0..WAVEFORM_BARS + 5 {
            assert!(waveform.push(&[0; 10], 10).is_none());
        }
        let bars = waveform.push(&sine(100, 0.1), WAVEFORM_EMIT_SAMPLES).unwrap();
        assert_eq!(bars.len(), WAVEFORM_BARS);
        assert_eq!(bars[..WAVEFORM_BARS - 1], [0.0; WAVEFORM_BARS - 1]);
        assert!(waveform.push(&[0; 10], 10).is_none());
    }

    #[test]
    fn waveform_scales_a_sine_by_root_rms() {
        let mut waveform = Waveform::new();
        let bars = waveform.push(&sine(1600, 0.1), WAVEFORM_EMIT_SAMPLES).unwrap();
        // RMS 0.1/sqrt(2) -> sqrt -> x2
        let expected = ((0.1f32 / 2f32.sqrt()).sqrt() * 2.0).clamp(0.0, 1.0);
        assert_eq!(bars.len(), 1);
        assert!((bars[0] - expected).abs() < 0.01, "{} vs {}", bars[0], expected);
        let loud = waveform.push(&sine(1600, 0.9), WAVEFORM_EMIT_SAMPLES).unwrap();
        assert_eq!(loud, vec![1.0]);
    }
}
