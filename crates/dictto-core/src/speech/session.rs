use crate::events;
use super::Action;
use crate::dictation::{emit_state, send_speech, BarAction};
use crate::host::{self, AppHandle, Emitter, Manager};

type AsyncJoinHandle = host::async_runtime::JoinHandle<()>;

/// Speech event routed through the dictation loop so it is ordered with key events.
pub(crate) enum Message {
    Action(Action),
    Limit(u64),
    Done(u64, Result<(), String>),
}

/// Speech sessions driven by the dictation loop. While one is active, dictation keys
/// are ignored and bar actions go to speech.
#[derive(Default)]
pub(crate) struct Sessions {
    session: Option<SpeechSession>,
    generation: u64,
}

impl Sessions {
    pub(crate) fn active(&self) -> bool {
        self.session.is_some()
    }
    /// `can_start` is false while dictation records or transcribes.
    pub(crate) async fn message(&mut self, app: &AppHandle, message: Message, can_start: bool) {
        let mut speech = self.session.take();
        match message {
            Message::Action(action) => {
                if let Some(session) = speech.take() {
                    if session.action == action {
                        speech = session.stop(app, false).await;
                    } else { speech = Some(session); }
                } else if can_start {
                    self.generation += 1;
                    speech = SpeechSession::start(action, self.generation, app);
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
        self.session = speech;
    }
    pub(crate) async fn bar(&mut self, app: &AppHandle, action: BarAction) {
        let Some(session) = self.session.take() else { return };
        self.session = match action {
            BarAction::Cancel => session.stop(app, true).await,
            BarAction::Stop => session.stop(app, false).await,
            BarAction::Start => Some(session),
        };
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
        let _ = app.emit(events::SPEECH_ERROR, serde_json::json!({"message":message}));
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
        let task = host::async_runtime::spawn(async move {
            tokio::time::sleep(duration).await;
            send_speech(Message::Limit(id));
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
        let task = host::async_runtime::spawn(async move {
            let result = action.run(app, audio).await;
            send_speech(Message::Done(id, result));
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
                .state::<crate::CoreState>()
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
