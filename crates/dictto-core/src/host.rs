use crate::CoreState;
use std::any::Any;
use std::sync::{Arc, Weak};

pub type EventSink = dyn Fn(&str, serde_json::Value) + Send + Sync;

#[derive(Clone)]
pub struct AppHandle {
    // Weak so that state holding a handle (the recorder) does not keep itself alive.
    states: Vec<Weak<dyn Any + Send + Sync>>,
    sink: Arc<EventSink>,
}

impl AppHandle {
    pub fn new(state: &Arc<CoreState>, sink: Arc<EventSink>) -> Self {
        Self { states: Vec::new(), sink }.with_state(state)
    }
    /// Registers app-owned state that `Manager::state` resolves next to `CoreState`.
    pub fn with_state<T: Any + Send + Sync>(mut self, state: &Arc<T>) -> Self {
        let state: Arc<dyn Any + Send + Sync> = state.clone();
        self.states.push(Arc::downgrade(&state));
        self
    }
}

pub trait Manager {
    fn state<T: Any + Send + Sync>(&self) -> Arc<T>;
}
impl Manager for AppHandle {
    fn state<T: Any + Send + Sync>(&self) -> Arc<T> {
        self.states
            .iter()
            .find_map(|state| state.upgrade()?.downcast().ok())
            .unwrap_or_else(|| panic!("unsupported application state"))
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
                .worker_threads(2)
                .thread_name("dictto-async")
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
            .expect("core Tokio runtime initialized")
            .spawn(future)
    }

    #[cfg(test)]
    #[test]
    fn background_runtime_uses_two_workers() {
        initialize().unwrap();
        assert_eq!(RUNTIME.get().unwrap().metrics().num_workers(), 2);
    }
}
