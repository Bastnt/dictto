//! Cuts a live 16 kHz mono capture into chunks at pauses so each chunk can be transcribed
//! while the user keeps speaking, and squeezes long silences out of every chunk.
//!
//! A chunk is only cut once enough has been recorded, and the pause needed to cut shrinks
//! as the recording grows: a short phrase waits through a pause, a long monologue is cut
//! at the next breath. Every window counts, speech or silence, the pause itself included,
//! so a chunk's length is the elapsed recording time.
use super::dsp::{SAMPLE_RATE, SILENCE_RMS_THRESHOLD};

const WINDOW: usize = SAMPLE_RATE as usize / 50;
const fn windows(ms: usize) -> usize {
    ms / 20
}

/// Below this much recording a chunk is never cut, whatever the pause.
const MIN_AUDIO: usize = windows(6_000);
/// Pause needed to cut at `MIN_AUDIO`, shrinking linearly to `SHORTEST_PAUSE` at `FULL_AUDIO`.
const LONGEST_PAUSE: usize = windows(4_000);
const SHORTEST_PAUSE: usize = windows(600);
const FULL_AUDIO: usize = windows(30_000);
/// Continuous speech or noise is cut here regardless, to bound one request.
const MAX_CHUNK: usize = windows(60_000);
/// Silence kept on each side of speech; longer pauses are shortened to twice this.
const PAD: usize = windows(250);
/// Chunks with less speech than this are clicks or breaths, not words.
const MIN_CHUNK_SPEECH: usize = windows(200);
/// Speech must stand this far above the noise floor...
const FLOOR_RATIO: f64 = 3.0;
/// ...unless that is louder than this share of the loudest window (-20 dB).
const PEAK_RATIO: f64 = 0.1;
const SMOOTHING: usize = windows(200);

pub fn required_pause(audio_windows: usize) -> Option<usize> {
    if audio_windows < MIN_AUDIO {
        return None;
    }
    let progress = (audio_windows - MIN_AUDIO).min(FULL_AUDIO - MIN_AUDIO);
    Some(LONGEST_PAUSE - (LONGEST_PAUSE - SHORTEST_PAUSE) * progress / (FULL_AUDIO - MIN_AUDIO))
}

pub struct Segmenter {
    partial: Vec<i16>,
    /// Window-aligned samples of the chunk being built, with each window's RMS.
    chunk: Vec<i16>,
    levels: Vec<f64>,
    trailing_silence: usize,
    recent: Vec<f64>,
    floor: f64,
    peak: f64,
}

impl Default for Segmenter {
    fn default() -> Self {
        Self {
            partial: Vec::new(),
            chunk: Vec::new(),
            levels: Vec::new(),
            trailing_silence: 0,
            recent: Vec::new(),
            floor: f64::INFINITY,
            peak: 0.0,
        }
    }
}

impl Segmenter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the chunks this packet completed, already compressed.
    pub fn push(&mut self, samples: &[i16]) -> Vec<Vec<i16>> {
        self.partial.extend_from_slice(samples);
        let whole = self.partial.len() / WINDOW * WINDOW;
        let ready: Vec<i16> = self.partial.drain(..whole).collect();
        ready.chunks(WINDOW).filter_map(|window| self.window(window)).collect()
    }

    /// The rest of the capture as a last chunk, or `None` when it holds no speech.
    pub fn finish(mut self) -> Option<Vec<i16>> {
        if !self.partial.is_empty() {
            let partial = std::mem::take(&mut self.partial);
            self.listen(&partial);
            self.chunk.extend_from_slice(&partial);
        }
        self.cut()
    }

    fn window(&mut self, window: &[i16]) -> Option<Vec<i16>> {
        let rms = self.listen(window);
        self.chunk.extend_from_slice(window);
        if rms >= self.threshold() {
            self.trailing_silence = 0;
        } else {
            self.trailing_silence += 1;
        }
        let paused = required_pause(self.levels.len()).is_some_and(|pause| self.trailing_silence >= pause);
        if paused || self.levels.len() >= MAX_CHUNK {
            return self.cut();
        }
        None
    }

    fn listen(&mut self, window: &[i16]) -> f64 {
        let rms = (window.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / window.len() as f64).sqrt();
        if self.recent.len() == SMOOTHING {
            self.recent.remove(0);
        }
        self.recent.push(rms);
        if self.recent.len() == SMOOTHING {
            self.floor = self.floor.min(self.recent.iter().sum::<f64>() / SMOOTHING as f64);
        }
        self.peak = self.peak.max(rms);
        self.levels.push(rms);
        rms
    }

    /// The floor is the quietest stretch heard so far, smoothed so the zeros a device emits
    /// while starting do not count as the room. Misjudging speech as silence loses words,
    /// so the threshold never rises above `PEAK_RATIO` of the loudest window: when speech
    /// barely clears the noise, the noise is kept rather than risk the speech.
    fn threshold(&self) -> f64 {
        SILENCE_RMS_THRESHOLD.max((self.floor * FLOOR_RATIO).min(self.peak * PEAK_RATIO))
    }

    /// Windows are judged again with what is known now, so the noise heard before the
    /// first word is recognised as such once the speech shows how loud the voice is.
    fn cut(&mut self) -> Option<Vec<i16>> {
        let chunk = std::mem::take(&mut self.chunk);
        let threshold = self.threshold();
        let speech: Vec<bool> = std::mem::take(&mut self.levels).iter().map(|&rms| rms >= threshold).collect();
        self.trailing_silence = 0;
        (speech.iter().filter(|&&s| s).count() >= MIN_CHUNK_SPEECH).then(|| compress(&chunk, &speech))
    }
}

/// Keeps every speech window and at most `PAD` silent windows next to speech.
fn compress(samples: &[i16], speech: &[bool]) -> Vec<i16> {
    let near_speech = |i: usize| {
        let start = i.saturating_sub(PAD);
        let end = (i + PAD + 1).min(speech.len());
        speech[start..end].iter().any(|&s| s)
    };
    let mut out = Vec::with_capacity(samples.len());
    for (i, window) in samples.chunks(WINDOW).enumerate() {
        if near_speech(i) {
            out.extend_from_slice(window);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: usize) -> Vec<i16> {
        (0..ms * SAMPLE_RATE as usize / 1000)
            .map(|i| ((i as f64 * 0.2).sin() * 3000.0) as i16)
            .collect()
    }

    fn silence(ms: usize) -> Vec<i16> {
        vec![0; ms * SAMPLE_RATE as usize / 1000]
    }

    fn feed(segmenter: &mut Segmenter, parts: &[Vec<i16>]) -> Vec<Vec<i16>> {
        parts.iter().flat_map(|part| part.chunks(480)).flat_map(|packet| segmenter.push(packet)).collect()
    }

    const PAD_MS: usize = PAD * 20;

    fn ms(samples: &[i16]) -> usize {
        samples.len() * 1000 / SAMPLE_RATE as usize
    }

    #[test]
    fn a_short_recording_waits_through_a_long_pause() {
        let mut segmenter = Segmenter::new();
        let chunks = feed(&mut segmenter, &[tone(1_000), silence(4_000), tone(1_000)]);
        assert!(chunks.is_empty());
        let last = segmenter.finish().unwrap();
        // Both phrases, the 4 s pause squeezed to two pads.
        assert_eq!(ms(&last), 1_000 + 2 * PAD_MS + 1_000);
    }

    #[test]
    fn the_pause_itself_counts_toward_the_recording_needed_to_cut() {
        let mut segmenter = Segmenter::new();
        // 1 s of speech then silence: cut once 6 s are recorded, the pause being 5 s by then.
        let chunks = feed(&mut segmenter, &[tone(1_000), silence(5_000)]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(ms(&chunks[0]), 1_000 + PAD_MS);
    }

    #[test]
    fn enough_speech_is_cut_at_a_long_pause_but_not_a_short_one() {
        let mut segmenter = Segmenter::new();
        assert!(feed(&mut segmenter, &[tone(9_000), silence(3_000), tone(500)]).is_empty());
        let chunks = feed(&mut segmenter, &[silence(3_600)]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(ms(&chunks[0]), 9_000 + 2 * PAD_MS + 500 + PAD_MS);
        assert!(segmenter.finish().is_none());
    }

    #[test]
    fn pauses_count_toward_the_audio_needed_to_cut() {
        let mut segmenter = Segmenter::new();
        // Only 4 s of speech, but 7 s of recording once the first pause is counted.
        assert!(feed(&mut segmenter, &[tone(2_000), silence(3_000), tone(2_000)]).is_empty());
        let chunks = feed(&mut segmenter, &[silence(4_000)]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(ms(&chunks[0]), 2_000 + 2 * PAD_MS + 2_000 + PAD_MS);
    }

    #[test]
    fn longer_speech_accepts_a_shorter_pause() {
        let mut segmenter = Segmenter::new();
        let chunks = feed(&mut segmenter, &[tone(31_000), silence(620)]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(required_pause(windows(6_000)), Some(windows(4_000)));
        assert_eq!(required_pause(windows(9_000)), Some(windows(3_580)));
        assert!(required_pause(windows(20_000)) < required_pause(windows(10_000)));
        assert_eq!(required_pause(windows(40_000)), Some(SHORTEST_PAUSE));
        assert_eq!(required_pause(windows(5_980)), None);
    }

    #[test]
    fn the_shortest_pause_is_reached_after_30_s_of_recording_not_of_speech() {
        let mut segmenter = Segmenter::new();
        // 30 s recorded, only 20 s of it speech, with pauses too short to cut.
        let breaths: Vec<Vec<i16>> = (0..20).flat_map(|_| [tone(1_000), silence(500)]).collect();
        assert!(feed(&mut segmenter, &breaths).is_empty());
        assert_eq!(feed(&mut segmenter, &[silence(120)]).len(), 1);
    }

    #[test]
    fn unbroken_speech_is_cut_at_the_cap() {
        let mut segmenter = Segmenter::new();
        let chunks = feed(&mut segmenter, &[tone(70_000)]);
        assert_eq!(chunks.len(), 1);
        assert_eq!(ms(&chunks[0]), 60_000);
        assert_eq!(ms(&segmenter.finish().unwrap()), 10_000);
    }

    #[test]
    fn silence_and_clicks_produce_no_chunk() {
        let mut segmenter = Segmenter::new();
        assert!(feed(&mut segmenter, &[silence(3_000), tone(100), silence(3_000)]).is_empty());
        assert!(segmenter.finish().is_none());
    }

    fn noise(ms: usize) -> Vec<i16> {
        (0..ms * SAMPLE_RATE as usize / 1000).map(|i| if i % 2 == 0 { 150 } else { -150 }).collect()
    }

    #[test]
    fn background_noise_between_phrases_is_squeezed() {
        let mut segmenter = Segmenter::new();
        assert!(feed(&mut segmenter, &[noise(40), tone(1_000), noise(4_000), tone(1_000)]).is_empty());
        assert_eq!(ms(&segmenter.finish().unwrap()), 40 + 1_000 + 2 * PAD_MS + 1_000);
    }

    #[test]
    fn noise_close_to_the_speech_level_is_kept() {
        let loud_noise: Vec<i16> = noise(5_000).iter().map(|s| s * 10).collect();
        let mut segmenter = Segmenter::new();
        feed(&mut segmenter, &[tone(1_000), loud_noise, tone(1_000)]);
        assert_eq!(ms(&segmenter.finish().unwrap()), 7_000);
    }

    #[test]
    fn device_startup_zeros_do_not_become_the_noise_floor() {
        let mut segmenter = Segmenter::new();
        feed(&mut segmenter, &[silence(40), noise(3_000), tone(1_000), noise(3_000), tone(1_000)]);
        // The noise is learned as the floor, so only its edges next to the words remain.
        assert_eq!(ms(&segmenter.finish().unwrap()), PAD_MS + 1_000 + 2 * PAD_MS + 1_000);
    }

    #[test]
    fn a_partial_window_still_reaches_the_last_chunk() {
        let mut segmenter = Segmenter::new();
        let mut audio = tone(500);
        audio.extend_from_slice(&tone(1)[..7]);
        assert!(segmenter.push(&audio).is_empty());
        assert_eq!(segmenter.finish().unwrap().len(), audio.len());
    }
}
