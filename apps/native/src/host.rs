use crate::AppState;
use std::any::Any;
use std::sync::{Arc, Weak};

pub type EventSink = dyn Fn(&str, serde_json::Value) + Send + Sync;

#[derive(Clone)]
pub struct AppHandle {
    state: Weak<AppState>,
    sink: Arc<EventSink>,
}

pub type App = AppHandle;

impl AppHandle {
    pub fn new(state: &Arc<AppState>, sink: Arc<EventSink>) -> Self {
        Self {
            state: Arc::downgrade(state),
            sink,
        }
    }
    pub fn handle(&self) -> &Self {
        self
    }
}

pub trait Manager {
    fn state<T: Any + Send + Sync>(&self) -> Arc<T>;
}
impl Manager for AppHandle {
    fn state<T: Any + Send + Sync>(&self) -> Arc<T> {
        let state: Arc<dyn Any + Send + Sync> = self
            .state
            .upgrade()
            .expect("native application state is alive");
        state
            .downcast()
            .unwrap_or_else(|_| panic!("unsupported native application state"))
    }
}

pub trait Emitter {
    fn emit<S: serde::Serialize>(&self, event: &str, payload: S) -> Result<(), serde_json::Error>;
}
impl Emitter for AppHandle {
    fn emit<S: serde::Serialize>(&self, event: &str, payload: S) -> Result<(), serde_json::Error> {
        (self.sink)(event, serde_json::to_value(payload)?);
        Ok(())
    }
}

pub mod async_runtime {
    use std::future::Future;
    use std::sync::OnceLock;
    pub use tokio::task::JoinHandle;
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    pub fn initialize() -> Result<(), String> {
        if RUNTIME.get().is_none() {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            let _ = RUNTIME.set(runtime);
        }
        Ok(())
    }
    pub fn spawn<F: Future + Send + 'static>(future: F) -> JoinHandle<F::Output>
    where
        F::Output: Send + 'static,
    {
        RUNTIME
            .get()
            .expect("native Tokio runtime initialized")
            .spawn(future)
    }
}
