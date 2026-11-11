use super::*;

pub(super) struct SpeechSession {
    pub(super) action: crate::speech::Action,
    pub(super) id: u64,
    pub(super) recording: bool,
    task: AsyncJoinHandle,
}

pub(super) fn speech_result(app: &tauri::AppHandle, result: Result<(), String>) {
    emit_state(app, "idle", None);
    if let Err(message) = result {
        let _ = app.emit("speech-error", serde_json::json!({"message":message}));
    }
}

impl SpeechSession {
    pub(super) fn start(
        action: crate::speech::Action,
        id: u64,
        app: &tauri::AppHandle,
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
        let tx = HOTKEY_TX.get()?.clone();
        let task = tauri::async_runtime::spawn(async move {
            tokio::time::sleep(duration).await;
            let _ = tx.send(HotkeyEvent::SpeechLimit(id));
        });
        Some(Self {
            action,
            id,
            recording: true,
            task,
        })
    }
    fn process(
        action: crate::speech::Action,
        id: u64,
        app: &tauri::AppHandle,
        audio: Option<Vec<u8>>,
    ) -> Self {
        emit_state(app, "processing", None);
        let app = app.clone();
        let tx = HOTKEY_TX.get().unwrap().clone();
        let task = tauri::async_runtime::spawn(async move {
            let result = action.run(app, audio).await;
            let _ = tx.send(HotkeyEvent::SpeechDone(id, result));
        });
        Self {
            action,
            id,
            recording: false,
            task,
        }
    }
    pub(super) async fn stop(self, app: &tauri::AppHandle, cancel: bool) -> Option<Self> {
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
