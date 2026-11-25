//! Push-to-talk dictation state machine, fed by any platform keyboard source.
use crate::events;
use crate::audio::DeviceSelection;
use crate::host::{self, AppHandle, Emitter, Manager};
use crate::keys;
use crate::live::LiveTranscript;
use crate::pipeline::RecordedAudio;
use crate::platform::{Sound, Sounds};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

/// Dictation chord state plus the host's input suspension. Suspension is kept apart from
/// `Tracker::suspended`, which settings capture owns, so neither clears the other.
#[derive(Default)]
struct Input {
    tracker: keys::Tracker,
    suspended: bool,
}

impl Input {
    /// `None` when suspended input drops the event.
    fn key(&mut self, source: u16, key: Option<keys::Key>, down: bool) -> Option<Option<bool>> {
        if self.suspended {
            return None;
        }
        Some(key.and_then(|key| self.tracker.event_from(source, key, down)))
    }
    /// `Some(false)` when a held chord is released by the reset.
    fn reset(&mut self) -> Option<bool> {
        self.tracker.reset()
    }
    fn set_suspended(&mut self, suspended: bool) -> Option<bool> {
        self.suspended = suspended;
        self.reset()
    }
}

static INPUT: OnceLock<Mutex<Input>> = OnceLock::new();

fn input() -> &'static Mutex<Input> {
    INPUT.get_or_init(Default::default)
}

fn send_transition(transition: Option<bool>) {
    if let (Some(pressed), Some(tx)) = (transition, HOTKEY_TX.get()) {
        let _ = tx.send(if pressed { HotkeyEvent::Pressed } else { HotkeyEvent::Released });
    }
}

pub fn configure(value: keys::Chord) {
    let mut input = input().lock().unwrap();
    send_transition(input.tracker.reset());
    input.tracker.chord = value;
}

pub fn set_capture(active: bool) {
    #[cfg(feature = "speech")]
    crate::speech::suspend(active);
    let mut input = input().lock().unwrap();
    send_transition(input.tracker.reset());
    input.tracker.suspended = active;
}

/// Forgets every held key, as when the keyboard that held them disappears. A dictation
/// hold in progress receives its release and stops normally.
pub fn reset_input() {
    let mut input = input().lock().unwrap();
    send_transition(input.reset());
    #[cfg(feature = "speech")]
    crate::speech::reset_input();
}

/// While suspended (locked or inactive session) key events are neither tracked nor
/// swallowed. Both edges reset held keys, since releases may be missed in between.
pub fn set_input_suspended(suspended: bool) {
    let mut input = input().lock().unwrap();
    send_transition(input.set_suspended(suspended));
    #[cfg(feature = "speech")]
    crate::speech::reset_input();
}

/// Feeds one key transition from the platform keyboard source. `source` is the platform
/// key code: it tells left and right modifiers apart and is what chords track keys by.
/// Unmapped keys (`key == None`) never affect the dictation chord but still reach speech.
/// Returns true when the event must be swallowed.
pub fn key_event(source: u16, key: Option<keys::Key>, down: bool) -> bool {
    handle_key_event(source, key, down, false)
}

/// Feeds keyboard events with hold and double-tap gestures for recording speech shortcuts.
#[cfg(feature = "speech")]
pub fn key_event_with_speech_gestures(source: u16, key: Option<keys::Key>, down: bool) -> bool {
    handle_key_event(source, key, down, true)
}

fn handle_key_event(source: u16, key: Option<keys::Key>, down: bool, speech_gestures: bool) -> bool {
    // Held across the speech handling so a concurrent suspension cannot interleave.
    let mut input = input().lock().unwrap();
    let Some(transition) = input.key(source, key, down) else {
        return false;
    };
    send_transition(transition);
    speech_key_event(source, key, down, speech_gestures)
}

#[cfg(feature = "speech")]
fn speech_key_event(source: u16, key: Option<keys::Key>, down: bool, gestures: bool) -> bool {
    let (message, swallow) = crate::speech::key_event(source, key, down, gestures);
    if let Some(message) = message {
        send_speech(message);
    }
    swallow
}

#[cfg(not(feature = "speech"))]
fn speech_key_event(_: u16, _: Option<keys::Key>, _: bool, _: bool) -> bool {
    false
}

#[cfg(feature = "speech")]
pub(crate) fn send_speech(message: crate::speech::session::Message) {
    if let Some(tx) = HOTKEY_TX.get() {
        let _ = tx.send(HotkeyEvent::Speech(message));
    }
}

type AsyncJoinHandle = host::async_runtime::JoinHandle<()>;

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
        events::RECORDING_STATE_CHANGED,
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
fn stop_recording_sync(app: &AppHandle, sounds: &dyn Sounds, live: Option<LiveTranscript>) -> Option<RecordedAudio> {
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
            Some(RecordedAudio { data, released_at: t, live })
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

/// Failed recordings kept for a tap to retry; past this the oldest is dropped.
const MAX_KEPT_FAILURES: usize = 20;

/// Every failure is kept until retried, whatever succeeds or fails after it.
fn keep_failed(kept: &mut VecDeque<RecordedAudio>, audio: RecordedAudio) {
    if kept.len() == MAX_KEPT_FAILURES {
        log::warn!("[hotkey] {} recordings are waiting for a retry; dropping the oldest", MAX_KEPT_FAILURES);
        kept.pop_front();
    }
    kept.push_back(audio);
}

fn take_retry_audio(kept: &mut VecDeque<RecordedAudio>, pending: usize) -> Option<RecordedAudio> {
    if pending > 0 { return None; }
    kept.pop_front().map(|mut audio| {
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
            events::DICTATION_ERROR,
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

/// Start audio recording with the configured device, transcribing it live as it goes.
/// Returns the recording limit in seconds, or None on failure.
fn start_recording(app: &AppHandle, sounds: &dyn Sounds, preload: bool, pending: usize,
    live: &mut Option<LiveTranscript>) -> Option<u64> {
    let app_state = app.state::<crate::CoreState>();

    let validation = {
        let db = app_state.db.lock().unwrap();
        let languages: Vec<String> = serde_json::from_str(&db.get_setting("languages").unwrap_or_default()).unwrap_or_default();
        crate::transcription::TranscriptionConfig::from_languages(&languages)
            .validate(&db.get_setting("transcription_model").unwrap_or_default())
    };
    if let Err(error) = validation {
        emit_pending_state(app, pending);
        let _ = app.emit(events::DICTATION_ERROR, serde_json::json!({"message": error.to_string()}));
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
    let (tap, transcript) = crate::live::start(app);
    let mut recorder = app_state.audio_recorder.lock().unwrap();
    if let Err(e) = recorder.start_with_tap(device_selection, tap) {
        drop(recorder);
        log::error!("[hotkey] Failed to start recording: {}", e);
        emit_pending_state(app, pending);
        let _ = app.emit(events::DICTATION_ERROR, serde_json::json!({"message":e.to_string()}));
        return None;
    }
    drop(recorder); // explicit drop to release mutex before play_sound acquires db mutex
    *live = Some(transcript);

    if preload { crate::warmup::schedule(app); }
    play_sound(Sound::Start, app, sounds);
    Some(limit_secs)
}

/// How long a press can last and still count as a "tap" for double-tap detection
pub(crate) const TAP_MAX_DURATION: std::time::Duration = std::time::Duration::from_millis(250);
/// Window after first tap release to detect second tap
pub(crate) const DOUBLE_TAP_WINDOW: std::time::Duration = std::time::Duration::from_millis(300);

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
    let app_handle = app.clone();
    let timer_handle: Arc<Mutex<Option<AsyncJoinHandle>>> = Arc::new(Mutex::new(None));

    host::async_runtime::spawn(async move {
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
        host::async_runtime::spawn(async move {
            while let Some(mut audio) = pipeline_rx.recv().await {
                let mut result = crate::pipeline::run_pipeline(&pipeline_app, &mut audio).await;
                // One quick second attempt rides out a busy worker while the user is still
                // waiting; longer retries are left to a tap, since a late paste would land
                // wherever the cursor is by then.
                if let Err(error @ crate::pipeline::PipelineError::TranscriptionFailed(_)) = &result {
                    log::warn!("[hotkey] {}; trying once more", error);
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    result = crate::pipeline::run_pipeline(&pipeline_app, &mut audio).await;
                }
                let failed = crate::pipeline::retryable_audio(audio, &result);
                if let Err(error) = &result {
                    log::error!("[hotkey] Pipeline error: {}", error);
                    let message = if failed.is_some() {
                        format!("{} The recording is kept: tap the shortcut to retry.", error)
                    } else {
                        error.to_string()
                    };
                    let _ = pipeline_app.emit(events::DICTATION_ERROR, serde_json::json!({"message": message}));
                }
                let _ = completed_tx.send(failed);
            }
        });
        let mut pending_pipelines = 0usize;
        let mut retry_audio: VecDeque<RecordedAudio> = VecDeque::new();
        let mut live: Option<LiveTranscript> = None;

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
                        (_, HotkeyEvent::Speech(_)) => {}
                        // --- IDLE ---
                        (State::Idle, HotkeyEvent::Pressed) => {
                            log::debug!("[hotkey] {} + Pressed → starting recording", state.name());
                            // Show pill BEFORE start_recording() for instant visual feedback.
                            // WASAPI init may take 15-40ms on re-init; user sees pill in <5ms.
                            emit_state(&app_handle, "recording", Some("hold"));

                            // EAGER START: begin recording immediately on key-down
                            if let Some(limit_secs) = start_recording(&app_handle, sounds, retry_audio.is_empty() && pending_pipelines == 0, pending_pipelines, &mut live) {

                                // Spawn recording limit timer
                                let timer_handle_clone = timer_handle.clone();
                                let limit_tx = timer_tx.clone();
                                let handle = host::async_runtime::spawn(async move {
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
                                tap_timer = Some(host::async_runtime::spawn(async move {
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
                            tap_timer = Some(host::async_runtime::spawn(async move {
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

                            let audio_data = stop_recording_sync(&app_handle, sounds, live.take());
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

                            let audio_data = stop_recording_sync(&app_handle, sounds, live.take());
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
                            live = None;
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
                            let audio_data = stop_recording_sync(&app_handle, sounds, live.take());
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
                            if let Some(limit_secs) = start_recording(&app_handle, sounds, true, pending_pipelines, &mut live) {
                                emit_state(&app_handle, "recording", Some("continuous"));

                                // Spawn recording limit timer
                                let timer_handle_clone = timer_handle.clone();
                                let limit_tx = timer_tx.clone();
                                let handle = host::async_runtime::spawn(async move {
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
                            let audio_data = stop_recording_sync(&app_handle, sounds, live.take());
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
                            live = None;
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
                    if let Some(audio) = failed_audio {
                        keep_failed(&mut retry_audio, audio);
                    }
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
        let mut kept = VecDeque::new();
        keep_failed(&mut kept, RecordedAudio::new(vec![1, 2, 3], std::time::Instant::now()));
        assert!(take_retry_audio(&mut kept, 1).is_none());
        assert_eq!(kept.len(), 1);
        assert_eq!(take_retry_audio(&mut kept, 0).unwrap().data, vec![1, 2, 3]);
        assert!(take_retry_audio(&mut kept, 0).is_none());
    }

    #[test]
    fn failures_are_kept_in_order_and_bounded() {
        let mut kept = VecDeque::new();
        for index in 0..=MAX_KEPT_FAILURES {
            keep_failed(&mut kept, RecordedAudio::new(vec![index as u8], std::time::Instant::now()));
        }
        assert_eq!(kept.len(), MAX_KEPT_FAILURES);
        assert_eq!(take_retry_audio(&mut kept, 0).unwrap().data, vec![1]);
        assert_eq!(take_retry_audio(&mut kept, 0).unwrap().data, vec![2]);
    }

    #[test]
    fn invalid_stored_hotkey_falls_back() {
        assert_eq!(hotkey_from_setting(Some("Ctrl+Banana")).display(), "Ctrl+Win");
    }

    fn held_chord() -> Input {
        let mut input = Input::default();
        input.tracker.chord = keys::Chord::parse("Ctrl+F8").unwrap();
        assert_eq!(input.key(0xa2, Some(keys::Key::Ctrl), true), Some(None));
        assert_eq!(input.key(0x77, Some(keys::Key::F(8)), true), Some(Some(true)));
        input
    }

    #[test]
    fn reset_releases_a_held_chord() {
        let mut input = held_chord();
        assert_eq!(input.reset(), Some(false));
        assert_eq!(input.reset(), None);
        // The forgotten Ctrl must be pressed again.
        assert_eq!(input.key(0x77, Some(keys::Key::F(8)), true), Some(None));
    }

    #[test]
    fn suspending_input_releases_the_chord_and_drops_keys() {
        let mut input = held_chord();
        assert_eq!(input.set_suspended(true), Some(false));
        assert_eq!(input.key(0xa2, Some(keys::Key::Ctrl), true), None);
        assert_eq!(input.key(0x77, Some(keys::Key::F(8)), true), None);
        assert_eq!(input.set_suspended(false), None);
        assert_eq!(input.key(0xa2, Some(keys::Key::Ctrl), true), Some(None));
        assert_eq!(input.key(0x77, Some(keys::Key::F(8)), true), Some(Some(true)));
    }

    #[test]
    fn capture_and_input_suspension_do_not_clobber_each_other() {
        let mut input = held_chord();
        input.reset();
        input.tracker.suspended = true;
        input.set_suspended(true);
        input.set_suspended(false);
        assert!(input.tracker.suspended);
        assert_eq!(input.key(0x77, Some(keys::Key::F(8)), true), Some(None));
        input.set_suspended(true);
        input.tracker.suspended = false;
        assert!(input.suspended);
        assert_eq!(input.key(0x77, Some(keys::Key::F(8)), true), None);
    }

    #[test]
    fn missing_hotkey_uses_default() {
        assert_eq!(hotkey_from_setting(None).display(), "Ctrl+Win");
    }
}
