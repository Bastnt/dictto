use dictto_core::audio::DeviceMonitorState;
use dictto_core::host::AppHandle;
use dictto_core::platform::Services;
use std::sync::Arc;

pub struct WindowsServices;

impl Services for WindowsServices {
    fn set_autostart(&self, enabled: bool) -> Result<(), String> {
        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            crate::autostart::enable_autostart(&exe.to_string_lossy())
        } else {
            crate::autostart::disable_autostart()
        }
    }
    fn install_input(&self) -> Result<(), String> {
        crate::keyboard::install()
    }
    fn spawn_device_monitor(
        &self,
        state: Arc<DeviceMonitorState>,
        app: AppHandle,
    ) -> std::sync::mpsc::Sender<()> {
        crate::device_monitor::spawn(state, app)
    }
}
