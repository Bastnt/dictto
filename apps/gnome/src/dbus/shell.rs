//! Client for the Shell extension: the clipboard is only reachable from inside the
//! compositor on Wayland.
use zbus::Connection;

#[zbus::proxy(
    interface = "com.dictto.Shell1",
    default_service = "com.dictto.Shell",
    default_path = "/com/dictto/Shell"
)]
pub trait Shell {
    /// Keeps every mimetype's bytes in the extension under the returned token.
    fn save_clipboard(&self) -> zbus::Result<u32>;
    fn restore_clipboard(&self, token: u32) -> zbus::Result<()>;
    fn set_text(&self, text: &str) -> zbus::Result<()>;
    /// `which` is "clipboard" or "primary".
    fn get_text(&self, which: &str) -> zbus::Result<(bool, String)>;
}

pub async fn connect(connection: &Connection) -> zbus::Result<ShellProxy<'static>> {
    ShellProxy::new(connection).await
}
