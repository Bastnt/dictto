//! Names of the events the runtime and platforms send to the host UI through the sink.

/// `{ "amplitudes": [f32] }`
pub const WAVEFORM_DATA: &str = "waveform-data";
/// `{ "state": "idle" | "recording" | "processing", "mode": "hold" | "continuous" | null }`
pub const RECORDING_STATE_CHANGED: &str = "recording-state-changed";
/// No payload.
pub const AUDIO_DEVICES_CHANGED: &str = "audio-devices-changed";
/// `{ "message": String }`
pub const DICTATION_ERROR: &str = "dictation-error";
/// `{ "message": String }`
pub const SPEECH_ERROR: &str = "speech-error";
/// `{ "key": String, "value": String }`
pub const SETTING_CHANGED: &str = "setting-changed";
/// `{ "generation": u64, "models": [..] }` or `{ "generation": u64, "error": String }`
pub const SPEECH_CATALOG: &str = "speech-catalog";
