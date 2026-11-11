use dictto_core::events;
use dictto_core::host;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use host::Emitter;

pub use dictto_core::audio::{DeviceMonitorState, DeviceSelection};

// Required for the #[implement] proc macro to resolve ::windows_core crate name
#[cfg(windows)]
extern crate windows_core;

static PLAYBACK_GENERATION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn playback_generation() -> u64 {
    PLAYBACK_GENERATION.load(Ordering::Acquire)
}

// ─── Windows-specific implementation ─────────────────────────────────────────

#[cfg(windows)]
use windows::Win32::Foundation::PROPERTYKEY;
#[cfg(windows)]
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, ERole, IMMDeviceEnumerator,
    IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator, DEVICE_STATE,
};
#[cfg(windows)]
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
#[cfg(windows)]
use windows::Win32::System::Com::StructuredStorage;
#[cfg(windows)]
use windows::Win32::System::Com::STGM_READ;
#[cfg(windows)]
use windows::core::{implement, GUID, PCWSTR, Result};

#[cfg(windows)]
const PKEY_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

/// COM callback implementation for device change notifications.
#[cfg(windows)]
#[implement(IMMNotificationClient)]
struct DeviceChangeNotifier {
    state: Arc<DeviceMonitorState>,
    app_handle: host::AppHandle,
}

#[cfg(windows)]
impl IMMNotificationClient_Impl for DeviceChangeNotifier_Impl {
    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        pwstrdefaultdeviceid: &PCWSTR,
    ) -> Result<()> {
        // Follow eConsole only — the "Dispositivo predeterminado" in Windows Sound Settings.
        // Same approach as Whispr Flow: auto-detect = Windows default input device.
        if flow == eRender && role == eConsole {
            PLAYBACK_GENERATION.fetch_add(1, Ordering::AcqRel);
        }
        if flow != eCapture || role != eConsole {
            return Ok(());
        }

        let id = if pwstrdefaultdeviceid.is_null() {
            None
        } else {
            unsafe { pwstrdefaultdeviceid.to_string().ok() }
        };

        log::debug!(
            "[DeviceMonitor] OnDefaultDeviceChanged: eConsole → {:?}",
            id
        );

        if let Some(ref id_str) = id {
            *self.state.last_device_id.lock().unwrap() = Some(id_str.clone());
            if let Ok(name) = resolve_friendly_name_by_id(id_str) {
                *self.state.last_device_name.lock().unwrap() = name;
            }
        }
        self.state.devices_changed.store(true, Ordering::Release);
        let _ = self.app_handle.emit(events::AUDIO_DEVICES_CHANGED, ());
        Ok(())
    }

    fn OnDeviceStateChanged(
        &self,
        _pwstrdeviceid: &PCWSTR,
        _dwnewstate: DEVICE_STATE,
    ) -> Result<()> {
        // Intentionally no log — Windows fires 10-20 of these per physical device change.
        // OnDefaultDeviceChanged (logged above) is the actionable event.
        self.state.devices_changed.store(true, Ordering::Release);
        let _ = self.app_handle.emit(events::AUDIO_DEVICES_CHANGED, ());
        Ok(())
    }

    fn OnDeviceAdded(&self, pwstrdeviceid: &PCWSTR) -> Result<()> {
        let id = unsafe { pwstrdeviceid.to_string().unwrap_or_default() };
        log::debug!("[DeviceMonitor] OnDeviceAdded: {}", id);
        self.state.devices_changed.store(true, Ordering::Release);
        let _ = self.app_handle.emit(events::AUDIO_DEVICES_CHANGED, ());
        Ok(())
    }

    fn OnDeviceRemoved(&self, pwstrdeviceid: &PCWSTR) -> Result<()> {
        let id = unsafe { pwstrdeviceid.to_string().unwrap_or_default() };
        log::debug!("[DeviceMonitor] OnDeviceRemoved: {}", id);
        self.state.devices_changed.store(true, Ordering::Release);
        let _ = self.app_handle.emit(events::AUDIO_DEVICES_CHANGED, ());
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _pwstrdeviceid: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> Result<()> {
        // Ignore property changes (volume, etc.)
        Ok(())
    }
}

/// RAII guard: unregisters the callback before the COM object is dropped.
/// Prevents use-after-free crash (windows-rs #2487).
#[cfg(windows)]
struct NotificationGuard {
    enumerator: IMMDeviceEnumerator,
    callback: IMMNotificationClient,
}

#[cfg(windows)]
impl Drop for NotificationGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .enumerator
                .UnregisterEndpointNotificationCallback(&self.callback);
        }
        log::debug!("[DeviceMonitor] Unregistered notification callback");
    }
}

/// RAII guard for CoUninitialize.
#[cfg(windows)]
struct ComUninitGuard;

#[cfg(windows)]
impl Drop for ComUninitGuard {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

/// Resolve a device's friendly name from its endpoint ID.
#[cfg(windows)]
fn resolve_friendly_name_by_id(device_id: &str) -> std::result::Result<String, String> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("CoCreateInstance: {}", e))?;

        let wide_id: Vec<u16> = device_id.encode_utf16().chain(std::iter::once(0)).collect();
        let pcwstr = PCWSTR(wide_id.as_ptr());

        let device = enumerator
            .GetDevice(pcwstr)
            .map_err(|e| format!("GetDevice: {}", e))?;

        let store = device
            .OpenPropertyStore(STGM_READ)
            .map_err(|e| format!("OpenPropertyStore: {}", e))?;

        let prop = store
            .GetValue(&PKEY_FRIENDLY_NAME)
            .map_err(|e| format!("GetValue: {}", e))?;

        let pwstr = StructuredStorage::PropVariantToStringAlloc(&prop)
            .map_err(|e| format!("PropVariantToStringAlloc: {}", e))?;

        let name = pwstr.to_string().unwrap_or_default();
        CoTaskMemFree(Some(pwstr.as_ptr() as *const std::ffi::c_void));
        Ok(name)
    }
}

/// Read the current default capture device's ID and friendly name.
/// Uses eConsole — the "Dispositivo predeterminado" in Windows Sound Settings.
#[cfg(windows)]
fn read_initial_default(enumerator: &IMMDeviceEnumerator) -> (Option<String>, String) {
    unsafe {
        let device = enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .ok();

        if let Some(dev) = device {
            let id = dev.GetId().ok().and_then(|pwstr| {
                let s = pwstr.to_string().unwrap_or_default();
                CoTaskMemFree(Some(pwstr.as_ptr() as *const std::ffi::c_void));
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            });

            let name = dev
                .OpenPropertyStore(STGM_READ)
                .ok()
                .and_then(|store| store.GetValue(&PKEY_FRIENDLY_NAME).ok())
                .and_then(|prop| StructuredStorage::PropVariantToStringAlloc(&prop).ok())
                .map(|pwstr| {
                    let n = pwstr.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(pwstr.as_ptr() as *const std::ffi::c_void));
                    n
                })
                .unwrap_or_default();

            (id, name)
        } else {
            (None, String::new())
        }
    }
}

/// Spawn the device monitor thread. Returns an mpsc::Sender<()> for shutdown signaling.
/// Caller stores this in AppState. Sending `()` or dropping the sender signals shutdown.
#[cfg(windows)]
pub fn spawn(
    state: Arc<DeviceMonitorState>,
    app_handle: host::AppHandle,
) -> std::sync::mpsc::Sender<()> {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<()>();

    std::thread::Builder::new()
        .name("device-monitor".into())
        .spawn(move || {
            if let Err(e) = monitor_thread_main(state, app_handle, cmd_rx) {
                log::error!("[DeviceMonitor] Thread fatal error: {}", e);
            }
        })
        .expect("Failed to spawn device-monitor thread");

    cmd_tx
}

#[cfg(windows)]
fn monitor_thread_main(
    state: Arc<DeviceMonitorState>,
    app_handle: host::AppHandle,
    shutdown_rx: std::sync::mpsc::Receiver<()>,
) -> std::result::Result<(), String> {
    unsafe {
        // MTA — callbacks arrive freely on RPC threads (no message pump needed)
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(|e| format!("CoInitializeEx: {}", e))?;
        let _com_guard = ComUninitGuard;

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("CoCreateInstance: {}", e))?;

        // Read initial default device
        let (initial_id, initial_name) = read_initial_default(&enumerator);
        log::info!(
            "[DeviceMonitor] Initial device: {:?} ({})",
            initial_id,
            initial_name
        );
        *state.last_device_id.lock().unwrap() = initial_id;
        *state.last_device_name.lock().unwrap() = initial_name;

        // Create and register the notification callback
        let notifier = DeviceChangeNotifier {
            state: state.clone(),
            app_handle,
        };
        let callback: IMMNotificationClient = notifier.into();

        enumerator
            .RegisterEndpointNotificationCallback(&callback)
            .map_err(|e| format!("RegisterEndpointNotificationCallback: {}", e))?;

        // RAII guard ensures Unregister happens before callback is dropped
        let _guard = NotificationGuard {
            enumerator,
            callback,
        };

        log::info!("[DeviceMonitor] Registered and waiting for events");

        // Block until shutdown signal or channel disconnect (0% CPU)
        let _ = shutdown_rx.recv();
        log::info!("[DeviceMonitor] Shutting down");

        // _guard drops here → UnregisterEndpointNotificationCallback
        // _com_guard drops here → CoUninitialize
    }
    Ok(())
}
