pub mod clipboard;

use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum InjectionError {
    #[error("Clipboard error: {0}")]
    ClipboardError(String),
    #[error("Keyboard simulation error: {0}")]
    KeyboardError(String),
}

pub async fn inject_text(text: &str, paste_delay_ms: u64) -> Result<Instant, InjectionError> {
    let saved = clipboard::save_clipboard();
    clipboard::set_clipboard_text(text)?;

    // Windows publishes clipboard text synchronously, so paste can start immediately.
    simulate_paste()?;
    let pasted_at = Instant::now();

    // Keep the text available until the target application has read it.
    tokio::time::sleep(Duration::from_millis(paste_delay_ms)).await;

    if let Some(content) = saved {
        if let Err(e) = clipboard::restore_clipboard(content) {
            log::warn!("Warning: Failed to restore clipboard: {}", e);
        }
    }

    Ok(pasted_at)
}

fn simulate_paste() -> Result<(), InjectionError> {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};

    let mut enigo =
        Enigo::new(&Settings::default()).map_err(|e| InjectionError::KeyboardError(e.to_string()))?;

    enigo
        .key(Key::Control, Direction::Press)
        .map_err(|e| InjectionError::KeyboardError(e.to_string()))?;
    enigo
        .key(Key::Unicode('v'), Direction::Click)
        .map_err(|e| InjectionError::KeyboardError(e.to_string()))?;
    enigo
        .key(Key::Control, Direction::Release)
        .map_err(|e| InjectionError::KeyboardError(e.to_string()))?;

    Ok(())
}
