use super::Action;
use crate::host::{self as tauri, AppHandle, Emitter, Manager};
use dictto_core::dictation::{emit_state, send_speech, BarAction, SpeechHooks, SpeechMessage};
use dictto_core::keys::Key;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

type AsyncJoinHandle = tauri::async_runtime::JoinHandle<()>;

enum Message {
    Action(Action),
    Limit(u64),
    Done(u64, Result<(), String>),
}

/// Speech shortcuts and sessions plugged into the core dictation loop. The loop calls
/// `message`, `bar` and `active` one at a time, so a session taken out across an await
/// is never observed missing.
#[derive(Default)]
pub(crate) struct Hooks {
    session: Mutex<Option<SpeechSession>>,
    generation: AtomicU64,
}

#[async_trait::async_trait]
impl SpeechHooks for Hooks {
    fn key_event(&self, source: u16, _key: Option<Key>, down: bool) -> (Option<SpeechMessage>, bool) {
        let (action, swallow) = super::key_event(source, down);
        (action.map(|action| Box::new(Message::Action(action)) as SpeechMessage), swallow)
    }
    fn suspend(&self, active: bool) {
        super::suspend(active);
    }
    fn active(&self) -> bool {
        self.session.lock().unwrap().is_some()
    }
    async fn message(&self, app: &AppHandle, message: SpeechMessage, can_start: bool) {
        let Ok(message) = message.downcast::<Message>() else { return };
        let mut speech = self.session.lock().unwrap().take();
        match *message {
            Message::Action(action) => {
                if let Some(session) = speech.take() {
                    if session.action == action {
                        speech = session.stop(app, false).await;
                    } else { speech = Some(session); }
                } else if can_start {
                    let id = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
                    speech = SpeechSession::start(action, id, app);
                }
            }
            Message::Limit(id) => {
                if speech.as_ref().is_some_and(|s| s.id == id && s.recording) {
                    speech = speech.take().unwrap().stop(app, false).await;
                }
            }
            Message::Done(id, result) => {
                if speech.as_ref().is_some_and(|s| s.id == id) {
                    speech.take();
                    speech_result(app, result);
                }
            }
        }
        *self.session.lock().unwrap() = speech;
    }
    async fn bar(&self, app: &AppHandle, action: BarAction) {
        let session = self.session.lock().unwrap().take();
        let Some(session) = session else { return };
        let speech = match action {
            BarAction::Cancel => session.stop(app, true).await,
            BarAction::Stop => session.stop(app, false).await,
            BarAction::Start => Some(session),
        };
        *self.session.lock().unwrap() = speech;
    }
}

struct SpeechSession {
    action: Action,
    id: u64,
    recording: bool,
    task: AsyncJoinHandle,
}

fn speech_result(app: &AppHandle, result: Result<(), String>) {
    emit_state(app, "idle", None);
    if let Err(message) = result {
        let _ = app.emit("speech-error", serde_json::json!({"message":message}));
    }
}

impl SpeechSession {
    fn start(
        action: Action,
        id: u64,
        app: &AppHandle,
    ) -> Option<Self> {
        if !action.records() {
            return Some(Self::process(action, id, app, None));
        }
        let duration = match action.start(app) {
            Ok(duration) => duration,
            Err(error) => {
                speech_result(app, Err(error));
                return None;
            }
        };
        emit_state(app, "recording", Some("continuous"));
        let task = tauri::async_runtime::spawn(async move {
            tokio::time::sleep(duration).await;
            send_speech(Box::new(Message::Limit(id)));
        });
        Some(Self {
            action,
            id,
            recording: true,
            task,
        })
    }
    fn process(
        action: Action,
        id: u64,
        app: &AppHandle,
        audio: Option<Vec<u8>>,
    ) -> Self {
        emit_state(app, "processing", None);
        let app = app.clone();
        let task = tauri::async_runtime::spawn(async move {
            let result = action.run(app, audio).await;
            send_speech(Box::new(Message::Done(id, result)));
        });
        Self {
            action,
            id,
            recording: false,
            task,
        }
    }
    async fn stop(self, app: &AppHandle, cancel: bool) -> Option<Self> {
        self.task.abort();
        let _ = self.task.await;
        if self.recording {
            let result = app
                .state::<dictto_core::CoreState>()
                .audio_recorder
                .lock()
                .unwrap()
                .stop();
            if !cancel {
                match result {
                    Ok(audio) => {
                        return Some(Self::process(self.action, self.id, app, Some(audio)))
                    }
                    Err(error) => {
                        speech_result(app, Err(error.to_string()));
                        return None;
                    }
                }
            }
        }
        emit_state(app, "idle", None);
        None
    }
}
