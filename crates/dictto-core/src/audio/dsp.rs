use super::AudioError;

pub const SAMPLE_RATE: u32 = 16000;
pub const CHANNELS: u16 = 1;
/// RMS threshold for silence detection — low enough for cheap/quiet microphones
pub const SILENCE_RMS_THRESHOLD: f64 = 50.0;

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
