pub mod device_monitor;
pub mod recorder;

#[derive(serde::Serialize, Clone, Debug)]
pub struct MicrophoneInfo {
    pub name: String,
    pub id: String,
    pub is_default: bool,
    pub form_factor: String,
}
