//! Push-to-talk dictation state machine, fed by any platform keyboard source.
use crate::audio::DeviceSelection;
use crate::host::{self as tauri, AppHandle, Emitter, Manager};
use crate::keys;
use crate::pipeline::RecordedAudio;
use crate::platform::{Sound, Sounds};
use std::sync::{Arc, Mutex, OnceLock};

static TRACKER: OnceLock<Mutex<keys::Tracker>> = OnceLock::new();

fn tracker() -> &'static Mutex<keys::Tracker> {
    TRACKER.get_or_init(|| Mutex::new(keys::Tracker::default()))
}

fn send_transition(transition: Option<bool>) {
    if let (Some(pressed), Some(tx)) = (transition, HOTKEY_TX.get()) {
        let _ = tx.send(if pressed { HotkeyEvent::Pressed } else { HotkeyEvent::Released });
    }
}

pub fn configure(value: keys::Chord) {
    let mut tracker = tracker().lock().unwrap();
    send_transition(tracker.reset());
    tracker.chord = value;
}

pub fn set_capture(active: bool) {
    #[cfg(feature = "speech")]
    crate::speech::suspend(active);
    let mut tracker = tracker().lock().unwrap();
    send_transition(tracker.reset());
    tracker.suspended = active;
}

/// Feeds one key transition from the platform keyboard source. `source` is the platform
/// key code: it tells left and right modifiers apart and is what chords track keys by.
/// Unmapped keys (`key == None`) never affect the dictation chord but still reach speech.
/// Returns true when the event must be swallowed.
pub fn key_event(source: u16, key: Option<keys::Key>, down: bool) -> bool {
    if let Some(key) = key {
        send_transition(tracker().lock().unwrap().event_from(source, key, down));
    }
    speech_key_event(source, key, down)
}

#[cfg(feature = "speech")]
fn speech_key_event(source: u16, key: Option<keys::Key>, down: bool) -> bool {
    let (action, swallow) = crate::speech::key_event(source, key, down);
    if let Some(action) = action {
        send_speech(crate::speech::session::Message::Action(action));
    }
    swallow
}

#[cfg(not(feature = "speech"))]
fn speech_key_event(_: u16, _: Option<keys::Key>, _: bool) -> bool {
    false
}

#[cfg(feature = "speech")]
pub(crate) fn send_speech(message: crate::speech::session::Message) {
    if let Some(tx) = HOTKEY_TX.get() {
        let _ = tx.send(HotkeyEvent::Speech(message));
    }
}

type AsyncJoinHandle = tauri::async_runtime::JoinHandle<()>;

enum HotkeyEvent {
    #[cfg(feature = "speech")]
    Speech(crate::speech::session::Message),
    Pressed,
    Released,
}

/// Frontend → Rust commands for bar buttons
pub enum BarAction {
    Start,
    Stop,
    Cancel,
}

static HOTKEY_TX: OnceLock<tokio::sync::mpsc::UnboundedSender<HotkeyEvent>> = OnceLock::new();

/// Emit structured recording state event to frontend.
/// Payload: { "state": "recording", "mode": "hold" } or { "state": "idle", "mode": null }
pub fn emit_state(app: &AppHandle, state: &str, mode: Option<&str>) {
    let _ = app.emit(
        "recording-state-changed",
        serde_json::json!({ "state": state, "mode": mode }),
    );
}

/// Play a sound effect if enabled in settings.
fn play_sound(sound: Sound, app: &AppHandle, sounds: &dyn Sounds) {
    let enabled = {
        let state = app.state::<crate::CoreState>();
        let db = state.db.lock().unwrap();
        db.get_setting("sound_effects_enabled")
            .map(|v| v == "true")
            .unwrap_or(true)
    };
    if enabled {
        sounds.play(sound);
    }
}

/// Synchronously stop the recorder and return WAV data.
/// Returns None for empty/short recordings or stop failures.
fn stop_recording_sync(app: &AppHandle, sounds: &dyn Sounds) -> Option<RecordedAudio> {
    let t = std::time::Instant::now();
    log::debug!("[hotkey] stop_recording_sync: acquiring recorder lock...");
    let app_state = app.state::<crate::CoreState>();
    let mut recorder = app_state.audio_recorder.lock().unwrap();
    log::debug!("[hotkey] stop_recording_sync: calling recorder.stop()...");
    let result = match recorder.stop() {
        Ok(data) => {
            log::info!(
                "[pipeline] Audio stop + WAV encode: {:?} ({:.1} KB)",
                t.elapsed(),
                data.len() as f64 / 1024.0
            );
            Some(RecordedAudio { data, released_at: t })
        }
        Err(crate::audio::AudioError::EmptyRecording) => {
            log::debug!("[hotkey] stop_recording_sync: empty recording (too short or silent)");
            None
        }
        Err(e) => {
            log::error!("[hotkey] Failed to stop recording: {}", e);
            None
        }
    };
    drop(recorder);
    log::debug!("[hotkey] stop_recording_sync: completed in {:?}", t.elapsed());
    play_sound(Sound::Stop, app, sounds);
    result
}

fn emit_pending_state(app: &AppHandle, pending: usize) {
    emit_state(app, if pending > 0 { "processing" } else { "idle" }, None);
}

fn take_retry_audio(audio: &mut Option<RecordedAudio>, pending: usize) -> Option<RecordedAudio> {
    if pending > 0 { return None; }
    audio.take().map(|mut audio| {
        audio.released_at = std::time::Instant::now();
        audio
    })
}

/// Queue every completed recording for serial transcription and clipboard injection.
fn enqueue_pipeline(
    app: &AppHandle,
    audio_data: Option<RecordedAudio>,
    pipeline_tx: &tokio::sync::mpsc::UnboundedSender<RecordedAudio>,
    pending: &mut usize,
) {
    let Some(data) = audio_data else {
        emit_pending_state(app, *pending);
        return;
    };

    if pipeline_tx.send(data).is_err() {
        log::error!("[hotkey] Pipeline worker stopped; recording could not be queued");
        let _ = app.emit(
            "dictation-error",
            serde_json::json!({"message": "The transcription queue is unavailable."}),
        );
        emit_pending_state(app, *pending);
        return;
    }
    *pending += 1;
    emit_state(app, "processing", None);
}

/// Cancel recording: stop the audio stream and discard everything.
fn cancel_recording(app: &AppHandle, sounds: &dyn Sounds) {
    let app_state = app.state::<crate::CoreState>();
    app_state.audio_recorder.lock().unwrap().cancel();
    // Recorder lock is released before play_sound acquires the db mutex.
    play_sound(Sound::Stop, app, sounds); // same stop sound for cancel per user decision
}

/// Start audio recording with the configured device.
/// Returns the recording limit in seconds, or None on failure.
fn start_recording(app: &AppHandle, sounds: &dyn Sounds, preload: bool, pending: usize) -> Option<u64> {
    let app_state = app.state::<crate::CoreState>();

    let validation = {
        let db = app_state.db.lock().unwrap();
        let languages: Vec<String> = serde_json::from_str(&db.get_setting("languages").unwrap_or_default()).unwrap_or_default();
        crate::transcription::TranscriptionConfig::from_languages(&languages)
            .validate(&db.get_setting("transcription_model").unwrap_or_default())
    };
    if let Err(error) = validation {
        emit_pending_state(app, pending);
        let _ = app.emit("dictation-error", serde_json::json!({"message": error.to_string()}));
        return None;
    }

    // Step 1: Read DB settings (lock db, then drop)
    let (device_setting, limit_secs) = {
        let db = app_state.db.lock().unwrap();
        let device = db.get_setting("microphone_device");
        let limit: u64 = db
            .get_setting("recording_limit_seconds")
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);
        (device, limit)
    };

    // Step 2: Read device_monitor BEFORE locking audio_recorder (lock ordering: db -> monitor -> recorder)
    // device_monitor is Arc<DeviceMonitorState> — get_device_id() briefly locks internal Mutex
    let device_selection = match device_setting.as_deref() {
        None | Some("auto-detect") => {
            let monitor_id = app_state.device_monitor.get_device_id();
            log::debug!("[hotkey] start_recording: auto-detect, monitor_id={:?}", monitor_id);
            DeviceSelection::AutoDetect(monitor_id)
        }
        Some(name) => {
            log::debug!("[hotkey] start_recording: ByName({})", name);
            DeviceSelection::ByName(name.to_string())
        }
    };

    log::debug!("[hotkey] start_recording: device={:?}, limit={}s", device_selection, limit_secs);

    // Step 3: Lock recorder and start (AFTER reading device_monitor)
    let mut recorder = app_state.audio_recorder.lock().unwrap();
    if let Err(e) = recorder.start(device_selection) {
        drop(recorder);
        log::error!("[hotkey] Failed to start recording: {}", e);
        emit_pending_state(app, pending);
        let _ = app.emit("dictation-error", serde_json::json!({"message":e.to_string()}));
        return None;
    }
    drop(recorder); // explicit drop to release mutex before play_sound acquires db mutex

    if preload { crate::warmup::schedule(app); }
    play_sound(Sound::Start, app, sounds);
    Some(limit_secs)
}

/// How long a press can last and still count as a "tap" for double-tap detection
const TAP_MAX_DURATION: std::time::Duration = std::time::Duration::from_millis(250);
/// Window after first tap release to detect second tap
const DOUBLE_TAP_WINDOW: std::time::Duration = std::time::Duration::from_millis(300);

const DEFAULT_HOTKEY: &str = "Ctrl+Win";

fn hotkey_from_setting(value: Option<&str>) -> keys::Chord {
    value
        .and_then(|value| keys::Chord::parse(value).ok())
        .unwrap_or_else(|| keys::Chord::parse(DEFAULT_HOTKEY).unwrap())
}

/// Configures the stored dictation shortcut and spawns the state machine.
/// Installs no OS hook: keyboard sources call `key_event`.
pub fn start(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<crate::CoreState>();
    let sounds = state.platform.sounds.clone();
    let value = state.db.lock().unwrap().get_setting("hotkey");
    configure(hotkey_from_setting(value.as_deref()));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<HotkeyEvent>();

    HOTKEY_TX
        .set(tx)
        .map_err(|_| "Hotkey channel already initialized")?;
    log::info!("Registered dictation shortcut {}", value.as_deref().unwrap_or(DEFAULT_HOTKEY));

    // Channel for bar button actions (stop/cancel from frontend)
    let (bar_tx, mut bar_rx) = tokio::sync::mpsc::unbounded_channel::<BarAction>();

    // Store bar_tx in a global so the Tauri command can reach it
    BAR_ACTION_TX
        .set(bar_tx)
        .map_err(|_| "Bar action channel already initialized")?;

    // Spawn async task to process hotkey events with state machine
    let app_handle = app.handle().clone();
    let timer_handle: Arc<Mutex<Option<AsyncJoinHandle>>> = Arc::new(Mutex::new(None));

    tauri::async_runtime::spawn(async move {
        // State machine
        enum State {
            Idle,
            PendingHold,       // Pressed, waiting to see if it's a tap or hold
            WaitingSecondTap,  // First tap done, waiting for second tap
            HoldRecording,     // Recording in hold mode (release to stop)
            ContinuousRecording, // Recording in continuous mode (press again to stop)
        }

        impl State {
            fn name(&self) -> &'static str {
                match self {
                    State::Idle => "Idle",
                    State::PendingHold => "PendingHold",
                    State::WaitingSecondTap => "WaitingSecondTap",
                    State::HoldRecording => "HoldRecording",
                    State::ContinuousRecording => "ContinuousRecording",
                }
            }
        }

        #[cfg(feature = "speech")]
        let mut speech = crate::speech::session::Sessions::default();
        let sounds = &*sounds;
        let mut state = State::Idle;
        // Timer for tap/hold discrimination and double-tap window
        let mut tap_timer: Option<AsyncJoinHandle> = None;
        // Channel for tap timer expiry notifications
        let (timer_tx, mut timer_rx) = tokio::sync::mpsc::unbounded_channel::<&'static str>();
        let (pipeline_tx, mut pipeline_rx) = tokio::sync::mpsc::unbounded_channel::<RecordedAudio>();
        let (completed_tx, mut completed_rx) = tokio::sync::mpsc::unbounded_channel::<Option<RecordedAudio>>();
        let pipeline_app = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(audio) = pipeline_rx.recv().await {
                let result = crate::pipeline::run_pipeline(&pipeline_app, &audio).await;
                match &result {
                    Ok(()) => {}
                    Err(error) => {
                        log::error!("[hotkey] Pipeline error: {}", error);
                        let _ = pipeline_app.emit(
                            "dictation-error",
                            serde_json::json!({"message": error.to_string()}),
                        );
                    }
                }
                let _ = completed_tx.send(crate::pipeline::retryable_audio(audio, &result));
            }
        });
        let mut pending_pipelines = 0usize;
        let mut retry_audio: Option<RecordedAudio> = None;

        loop {
            tokio::select! {
                hotkey_event = rx.recv() => {
                    let Some(event) = hotkey_event else { break };

                    #[cfg(feature = "speech")]
                    match event {
                        HotkeyEvent::Speech(message) => {
                            let can_start = matches!(state, State::Idle) && pending_pipelines == 0;
                            speech.message(&app_handle, message, can_start).await;
                            continue;
                        }
                        _ if speech.active() => continue,
                        _ => {}
                    }
                    match (&state, event) {
                        #[cfg(feature = "speech")]
                        (_, HotkeyEvent::Speech(_)) => unreachable!(),
                        // --- IDLE ---
                        (State::Idle, HotkeyEvent::Pressed) => {
                            log::debug!("[hotkey] {} + Pressed → starting recording", state.name());
                            // Show pill BEFORE start_recording() for instant visual feedback.
                            // WASAPI init may take 15-40ms on re-init; user sees pill in <5ms.
                            emit_state(&app_handle, "recording", Some("hold"));

                            // EAGER START: begin recording immediately on key-down
                            if let Some(limit_secs) = start_recording(&app_handle, sounds, retry_audio.is_none() && pending_pipelines == 0, pending_pipelines) {

                                // Spawn recording limit timer
                                let timer_handle_clone = timer_handle.clone();
                                let limit_tx = timer_tx.clone();
                                let handle = tauri::async_runtime::spawn(async move {
                                    tokio::time::sleep(std::time::Duration::from_secs(limit_secs)).await;
                                    log::info!("Recording limit reached ({}s), auto-stopping", limit_secs);
                                    {
                                        let mut th = timer_handle_clone.lock().unwrap();
                                        *th = None;
                                    }
                                    let _ = limit_tx.send("limit_expired");
                                });
                                {
                                    let mut th = timer_handle.lock().unwrap();
                                    *th = Some(handle);
                                }

                                // Start tap/hold discrimination timer
                                if let Some(t) = tap_timer.take() { t.abort(); }
                                let tx = timer_tx.clone();
                                tap_timer = Some(tauri::async_runtime::spawn(async move {
                                    tokio::time::sleep(TAP_MAX_DURATION).await;
                                    let _ = tx.send("hold_confirmed");
                                }));

                                state = State::PendingHold;
                            }
                        }
                        (State::Idle, HotkeyEvent::Released) => {
                            // Spurious release, ignore
                        }

                        // --- PENDING HOLD ---
                        (State::PendingHold, HotkeyEvent::Released) => {
                            log::debug!("[hotkey] {} + Released → waiting for second tap", state.name());
                            // Released quickly = this was a tap. Wait for second tap.
                            // Recording KEEPS RUNNING — do NOT stop it.
                            if let Some(t) = tap_timer.take() { t.abort(); }
                            let tx = timer_tx.clone();
                            tap_timer = Some(tauri::async_runtime::spawn(async move {
                                tokio::time::sleep(DOUBLE_TAP_WINDOW).await;
                                let _ = tx.send("single_tap_expired");
                            }));
                            state = State::WaitingSecondTap;
                        }
                        (State::PendingHold, HotkeyEvent::Pressed) => {
                            // Shouldn't happen (already pressed), ignore
                        }

                        // --- WAITING SECOND TAP ---
                        (State::WaitingSecondTap, HotkeyEvent::Pressed) => {
                            log::debug!("[hotkey] {} + Pressed → continuous mode", state.name());
                            // Second tap detected → continuous mode!
                            // Recording already running from Idle→PendingHold.
                            // Recording limit timer already running too.
                            if let Some(t) = tap_timer.take() { t.abort(); }

                            // Switch UI mode from "hold" to "continuous"
                            emit_state(&app_handle, "recording", Some("continuous"));

                            state = State::ContinuousRecording;
                        }
                        (State::WaitingSecondTap, HotkeyEvent::Released) => {
                            // Release of second tap in continuous mode, or spurious — ignore
                            // (we already started recording on Pressed)
                        }

                        // --- HOLD RECORDING ---
                        (State::HoldRecording, HotkeyEvent::Released) => {
                            log::debug!("[hotkey] {} + Released → stopping recording", state.name());
                            // Cancel recording limit timer
                            {
                                let mut th = timer_handle.lock().unwrap();
                                if let Some(handle) = th.take() {
                                    handle.abort();
                                }
                            }

                            let audio_data = stop_recording_sync(&app_handle, sounds);
                            state = State::Idle;
                            enqueue_pipeline(&app_handle, audio_data, &pipeline_tx, &mut pending_pipelines);
                        }
                        (State::HoldRecording, HotkeyEvent::Pressed) => {
                            // Shouldn't happen (already holding), ignore
                        }

                        // --- CONTINUOUS RECORDING ---
                        (State::ContinuousRecording, HotkeyEvent::Pressed) => {
                            log::debug!("[hotkey] {} + Pressed → stopping recording", state.name());
                            // Hotkey pressed again → stop and process
                            {
                                let mut th = timer_handle.lock().unwrap();
                                if let Some(handle) = th.take() {
                                    handle.abort();
                                }
                            }

                            let audio_data = stop_recording_sync(&app_handle, sounds);
                            state = State::Idle;
                            enqueue_pipeline(&app_handle, audio_data, &pipeline_tx, &mut pending_pipelines);
                        }
                        (State::ContinuousRecording, HotkeyEvent::Released) => {
                            // Release after pressing to stop — already handled on Pressed, ignore
                        }
                    }
                }

                timer_event = timer_rx.recv() => {
                    let Some(event) = timer_event else { break };

                    match (&state, event) {
                        (State::PendingHold, "hold_confirmed") => {
                            log::debug!("[hotkey] {} + hold_confirmed → HoldRecording", state.name());
                            // User held long enough → confirmed hold mode.
                            // Recording already running from Idle→PendingHold.
                            // Recording limit timer already running too.
                            // UI already showing "recording"/"hold" from key-down.
                            tap_timer = None;
                            state = State::HoldRecording;
                        }
                        (State::WaitingSecondTap, "single_tap_expired") => {
                            log::debug!("[hotkey] {} + single_tap_expired → cancelling recording", state.name());
                            // No second tap came → cancel the eagerly-started recording
                            tap_timer = None;

                            // Abort the recording limit timer
                            {
                                let mut th = timer_handle.lock().unwrap();
                                if let Some(handle) = th.take() {
                                    handle.abort();
                                }
                            }

                            // Stop WASAPI stream and discard audio buffer
                            cancel_recording(&app_handle, sounds);
                            state = State::Idle;
                            if let Some(audio) = take_retry_audio(&mut retry_audio, pending_pipelines) {
                                enqueue_pipeline(&app_handle, Some(audio), &pipeline_tx, &mut pending_pipelines);
                            } else {
                                emit_pending_state(&app_handle, pending_pipelines);
                            }
                        }
                        (State::PendingHold, "limit_expired")
                        | (State::WaitingSecondTap, "limit_expired")
                        | (State::HoldRecording, "limit_expired")
                        | (State::ContinuousRecording, "limit_expired") => {
                            log::debug!("[hotkey] {} + limit_expired → stopping recording", state.name());
                            // Recording limit reached — stop and process
                            tap_timer = None;
                            let audio_data = stop_recording_sync(&app_handle, sounds);
                            state = State::Idle;
                            enqueue_pipeline(&app_handle, audio_data, &pipeline_tx, &mut pending_pipelines);
                        }
                        _ => {
                            // Stale timer event from a previous state, ignore
                        }
                    }
                }

                bar_event = bar_rx.recv() => {
                    let Some(action) = bar_event else { break };

                    #[cfg(feature = "speech")]
                    if speech.active() {
                        speech.bar(&app_handle, action).await;
                        continue;
                    }
                    match (&state, action) {
                        (State::Idle, BarAction::Start) => {
                            // UI click → start continuous recording directly
                            if let Some(limit_secs) = start_recording(&app_handle, sounds, true, pending_pipelines) {
                                emit_state(&app_handle, "recording", Some("continuous"));

                                // Spawn recording limit timer
                                let timer_handle_clone = timer_handle.clone();
                                let limit_tx = timer_tx.clone();
                                let handle = tauri::async_runtime::spawn(async move {
                                    tokio::time::sleep(std::time::Duration::from_secs(limit_secs)).await;
                                    log::info!("Recording limit reached ({}s), auto-stopping", limit_secs);
                                    {
                                        let mut th = timer_handle_clone.lock().unwrap();
                                        *th = None;
                                    }
                                    let _ = limit_tx.send("limit_expired");
                                });
                                {
                                    let mut th = timer_handle.lock().unwrap();
                                    *th = Some(handle);
                                }

                                state = State::ContinuousRecording;
                            }
                        }
                        (State::ContinuousRecording, BarAction::Stop) => {
                            // Stop button → process
                            {
                                let mut th = timer_handle.lock().unwrap();
                                if let Some(handle) = th.take() {
                                    handle.abort();
                                }
                            }
                            let audio_data = stop_recording_sync(&app_handle, sounds);
                            state = State::Idle;
                            enqueue_pipeline(&app_handle, audio_data, &pipeline_tx, &mut pending_pipelines);
                        }
                        (State::ContinuousRecording, BarAction::Cancel) => {
                            // Cancel button → discard
                            {
                                let mut th = timer_handle.lock().unwrap();
                                if let Some(handle) = th.take() {
                                    handle.abort();
                                }
                            }
                            cancel_recording(&app_handle, sounds);
                            state = State::Idle;
                            emit_pending_state(&app_handle, pending_pipelines);
                        }
                        _ => {
                            // Bar action in wrong state, ignore
                        }
                    }
                }

                pipeline_completed = completed_rx.recv() => {
                    let Some(failed_audio) = pipeline_completed else {
                        log::error!("[hotkey] Pipeline worker stopped");
                        break;
                    };
                    retry_audio = failed_audio;
                    pending_pipelines = pending_pipelines.saturating_sub(1);
                    if matches!(state, State::Idle) {
                        emit_pending_state(&app_handle, pending_pipelines);
                    }
                }
            }
        }
    });

    Ok(())
}

// Global channel for bar button actions
static BAR_ACTION_TX: OnceLock<tokio::sync::mpsc::UnboundedSender<BarAction>> = OnceLock::new();

/// Called by the frontend pill click to start continuous recording
pub fn bar_start() {
    if let Some(tx) = BAR_ACTION_TX.get() {
        let _ = tx.send(BarAction::Start);
    }
}

/// Called by the frontend "Stop" button in continuous mode
pub fn bar_stop() {
    if let Some(tx) = BAR_ACTION_TX.get() {
        let _ = tx.send(BarAction::Stop);
    }
}

/// Called by the frontend "Cancel" button in continuous mode
pub fn bar_cancel() {
    if let Some(tx) = BAR_ACTION_TX.get() {
        let _ = tx.send(BarAction::Cancel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_waits_for_pending_transcriptions_and_cannot_be_queued_twice() {
        let mut audio = Some(RecordedAudio {
            data: vec![1, 2, 3],
            released_at: std::time::Instant::now(),
        });
        assert!(take_retry_audio(&mut audio, 1).is_none());
        assert!(audio.is_some());
        assert_eq!(take_retry_audio(&mut audio, 0).unwrap().data, vec![1, 2, 3]);
        assert!(take_retry_audio(&mut audio, 0).is_none());
    }

    #[test]
    fn invalid_stored_hotkey_falls_back() {
        assert_eq!(hotkey_from_setting(Some("Ctrl+Banana")).display(), "Ctrl+Win");
    }

    #[test]
    fn missing_hotkey_uses_default() {
        assert_eq!(hotkey_from_setting(None).display(), "Ctrl+Win");
    }
}
