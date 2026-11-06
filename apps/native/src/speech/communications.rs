use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::path::Path;
use windows::core::{IUnknown_Vtbl, Interface, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Devices::{DeviceAndDriverInstallation::*, Properties::*};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

pub(super) trait Defaults {
    fn current(&self) -> Result<String, String>;
    fn target(&self) -> Result<String, String>;
    fn set(&self, id: &str) -> Result<(), String>;
    fn owns_route(&self, id: &str) -> Result<bool, String> {
        Ok(self.target()? == id)
    }
}

struct Apartment(bool);
impl Apartment {
    fn open() -> Result<Self, String> {
        let h = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if h.is_ok() {
            Ok(Self(true))
        } else if h == windows::Win32::Foundation::RPC_E_CHANGED_MODE {
            Ok(Self(false))
        } else {
            Err(format!("Cannot initialize Windows audio: {h:?}"))
        }
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() }
        }
    }
}

windows_core::imp::define_interface!(Policy, PolicyVtable, 0xf8679f50_850a_41cf_9c72_430f290290c8);
#[repr(C)]
pub struct PolicyVtable {
    base: IUnknown_Vtbl,
    // IPolicyConfig has ten format, period, share-mode and property methods before SetDefaultEndpoint.
    preceding: [usize; 10],
    set_default: unsafe extern "system" fn(*mut std::ffi::c_void, PCWSTR, ERole) -> HRESULT,
}
const POLICY_CLASS: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

pub(super) struct WindowsDefaults;
fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}
unsafe fn endpoint_id(device: &IMMDevice) -> windows::core::Result<String> {
    let ptr: PWSTR = device.GetId()?;
    let result = ptr.to_string();
    CoTaskMemFree(Some(ptr.0.cast()));
    Ok(result?)
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

unsafe fn belongs_to_driver(id: &str, hardware_id: &str) -> bool {
    // Endpoint devnodes are children of the audio adapter. Friendly names can be renamed or duplicated.
    let instance = wide(&format!("SWD\\MMDEVAPI\\{id}"));
    let mut node = 0;
    if CM_Locate_DevNodeW(
        &mut node,
        PCWSTR(instance.as_ptr()),
        CM_LOCATE_DEVNODE_NORMAL,
    ) != CR_SUCCESS
    {
        return false;
    }
    for _ in 0..32 {
        let mut kind = DEVPROPTYPE::default();
        let mut len = 0;
        if CM_Get_DevNode_PropertyW(
            node,
            &DEVPKEY_Device_HardwareIds,
            &mut kind,
            None,
            &mut len,
            0,
        ) == CR_BUFFER_SMALL
            && (2..=65536).contains(&len)
        {
            let mut buffer = vec![0u16; (len as usize).div_ceil(2)];
            if CM_Get_DevNode_PropertyW(
                node,
                &DEVPKEY_Device_HardwareIds,
                &mut kind,
                Some(buffer.as_mut_ptr().cast()),
                &mut len,
                0,
            ) == CR_SUCCESS
                && kind == DEVPROP_TYPE_STRING_LIST
            {
                if buffer
                    .split(|c| *c == 0)
                    .any(|s| String::from_utf16_lossy(s).eq_ignore_ascii_case(hardware_id))
                {
                    return true;
                }
            }
        }
        let mut parent = 0;
        if CM_Get_Parent(&mut parent, node, 0) != CR_SUCCESS {
            break;
        }
        node = parent;
    }
    false
}
unsafe fn branded_capture(device: &IMMDevice) -> bool {
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToUInt32;
    use windows::Win32::System::Variant::VT_UI4;
    let key = PROPERTYKEY {
        fmtid: GUID::from_u128(0x4f3ec4fb_1178_4adb_9313_8f584b69fb2d),
        pid: 2,
    };
    let Ok(value) = device
        .OpenPropertyStore(STGM_READ)
        .and_then(|store| store.GetValue(&key))
    else {
        return false;
    };
    value.Anonymous.Anonymous.vt == VT_UI4 && PropVariantToUInt32(&value).is_ok_and(|v| v == 1)
}

pub(super) fn cable_endpoint(flow: EDataFlow) -> Result<String, String> {
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    const DEVICE_DESCRIPTION: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 2,
    };
    let _apartment = Apartment::open()?;
    unsafe {
        let devices = enumerator()
            .and_then(|e| e.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE))
            .map_err(|e| e.to_string())?;
        let mut found = None;
        for i in 0..devices.GetCount().map_err(|e| e.to_string())? {
            let device = devices.Item(i).map_err(|e| e.to_string())?;
            let id = endpoint_id(&device).map_err(|e| e.to_string())?;
            if !belongs_to_driver(&id, "VBAudioVACWDM") {
                continue;
            }
            if flow == eRender {
                let value = device
                    .OpenPropertyStore(STGM_READ)
                    .and_then(|store| store.GetValue(&DEVICE_DESCRIPTION))
                    .map_err(|e| e.to_string())?;
                let name = PropVariantToStringAlloc(&value).map_err(|e| e.to_string())?;
                let label = name.to_string();
                CoTaskMemFree(Some(name.0.cast()));
                if label
                    .map_err(|e| e.to_string())?
                    .to_ascii_lowercase()
                    .starts_with("cable in 16")
                {
                    continue;
                }
            }
            if found.replace(id).is_some() {
                return Err(
                    "Multiple VB-CABLE endpoints match. Keep one standalone VB-CABLE installation."
                        .into(),
                );
            }
        }
        found.ok_or_else(|| "VB-CABLE is not available. Install it and enable both cable endpoints in Windows Sound.".into())
    }
}

impl Defaults for WindowsDefaults {
    fn current(&self) -> Result<String, String> {
        let _apartment = Apartment::open()?;
        unsafe {
            let device = enumerator()
                .map_err(|e| e.to_string())?
                .GetDefaultAudioEndpoint(eCapture, eCommunications)
                .map_err(|e| {
                    format!("Windows has no available default communications microphone ({e}).")
                })?;
            endpoint_id(&device).map_err(|e| e.to_string())
        }
    }
    fn target(&self) -> Result<String, String> {
        cable_endpoint(eCapture)
    }
    fn owns_route(&self, id: &str) -> Result<bool, String> {
        let _apartment = Apartment::open()?;
        unsafe {
            let name = wide(id);
            let device = enumerator()
                .and_then(|e| e.GetDevice(PCWSTR(name.as_ptr())))
                .map_err(|e| e.to_string())?;
            Ok(belongs_to_driver(id, "VBAudioVACWDM")
                || belongs_to_driver(id, r"ROOT\DICTTOMIC")
                || branded_capture(&device))
        }
    }
    fn set(&self, id: &str) -> Result<(), String> {
        let _apartment = Apartment::open()?;
        unsafe {
            let endpoint_name = wide(id);
            let device=enumerator().and_then(|e|e.GetDevice(PCWSTR(endpoint_name.as_ptr())))
                .map_err(|e|format!("The saved microphone is unavailable. Reconnect it or choose another Windows communications input ({e})."))?;
            let endpoint: IMMEndpoint = device.cast().map_err(|e| e.to_string())?;
            if endpoint.GetDataFlow().map_err(|e| e.to_string())? != eCapture
                || device.GetState().map_err(|e| e.to_string())? != DEVICE_STATE_ACTIVE
            {
                return Err(
                    "The saved communications microphone is not an active capture device.".into(),
                );
            }
            let client: Policy = CoCreateInstance(&POLICY_CLASS, None, CLSCTX_ALL)
                .map_err(|e| format!("Windows communications policy is unavailable: {e}"))?;
            (Interface::vtable(&client).set_default)(
                client.as_raw(),
                PCWSTR(endpoint_name.as_ptr()),
                eCommunications,
            )
            .ok()
            .map_err(|e| format!("Cannot change the default communications microphone: {e}"))
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Saved {
    original: String,
    temporary: String,
}

fn observed_after_set(defaults: &impl Defaults, previous: &str) -> Result<String, String> {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let current = defaults.current()?;
        if current != previous {
            return Ok(current);
        }
        if std::time::Instant::now() >= until {
            return Err("Windows has not confirmed the communications microphone change.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

pub(super) fn recover(path: &Path, defaults: &impl Defaults) -> Result<(), String> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let pending = path.with_extension("pending");
            if pending.exists() {
                fs::remove_file(pending).map_err(|e| e.to_string())?;
            }
            return Ok(());
        }
        Err(e) => return Err(format!("Cannot read microphone recovery file: {e}")),
    };
    let saved: Saved = serde_json::from_slice(&data)
        .map_err(|e| format!("Invalid microphone recovery file: {e}"))?;
    if defaults.current()? == saved.temporary {
        if !defaults.owns_route(&saved.temporary)? {
            return Err("The journaled device is not a supported virtual microphone.".into());
        }
        defaults.set(&saved.original)?;
        observed_after_set(defaults, &saved.temporary)?;
    }
    fs::remove_file(path).map_err(|e| format!("Cannot clear microphone recovery file: {e}"))
}

pub(super) fn acquire(path: &Path, defaults: &impl Defaults) -> Result<String, String> {
    recover(path, defaults)?;
    let target = defaults.target()?;
    let original = defaults.current()?;
    if original == target {
        return Ok(target);
    }
    let saved = Saved {
        original,
        temporary: target.clone(),
    };
    let pending = path.with_extension("pending");
    {
        let file = File::create(&pending).map_err(|e| e.to_string())?;
        serde_json::to_writer(&file, &saved).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    fs::rename(&pending, path).map_err(|e| e.to_string())?;
    let current = defaults.current();
    if current.as_ref().ok() != Some(&saved.original) {
        // No policy write was issued; the endpoint can disappear or be selected manually here.
        fs::remove_file(path).map_err(|e| e.to_string())?;
        return Err(current.err().unwrap_or_else(|| {
            "The communications microphone changed before routing began.".into()
        }));
    }
    if let Err(error) = defaults.set(&target) {
        if let Err(restore) = recover(path, defaults) {
            return Err(format!("{error} Recovery pending: {restore}"));
        }
        return Err(error);
    }
    let observed = observed_after_set(defaults, &saved.original)
        .map_err(|e| format!("{e} The recovery file was retained."))?;
    if observed != target {
        fs::remove_file(path).map_err(|e| e.to_string())?;
        return Err("The user selected a different communications microphone.".into());
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    struct Fake(RefCell<String>);
    impl Defaults for Fake {
        fn current(&self) -> Result<String, String> {
            Ok(self.0.borrow().clone())
        }
        fn target(&self) -> Result<String, String> {
            Ok("virtual".into())
        }
        fn set(&self, id: &str) -> Result<(), String> {
            *self.0.borrow_mut() = id.into();
            Ok(())
        }
    }
    #[test]
    fn missing_prior_input_never_attempts_a_switch() {
        struct Missing;
        impl Defaults for Missing {
            fn current(&self) -> Result<String, String> {
                Err("No default capture endpoint".into())
            }
            fn target(&self) -> Result<String, String> {
                Ok("virtual".into())
            }
            fn set(&self, _: &str) -> Result<(), String> {
                panic!("No restorable microphone")
            }
        }
        let path =
            std::env::temp_dir().join(format!("dictto-missing-mic-{}.json", std::process::id()));
        assert!(acquire(&path, &Missing).is_err());
        assert!(!path.exists());
    }
    #[test]
    fn user_selecting_virtual_before_first_policy_write_is_preserved() {
        struct Changed(std::cell::Cell<u8>);
        impl Defaults for Changed {
            fn current(&self) -> Result<String, String> {
                let n = self.0.get();
                self.0.set(n + 1);
                Ok(if n == 0 { "usb" } else { "virtual" }.into())
            }
            fn target(&self) -> Result<String, String> {
                Ok("virtual".into())
            }
            fn set(&self, _: &str) -> Result<(), String> {
                panic!("No switch was authorized after manual selection")
            }
        }
        let path =
            std::env::temp_dir().join(format!("dictto-user-mic-{}.json", std::process::id()));
        assert!(acquire(&path, &Changed(0.into())).is_err());
        assert!(!path.exists());
    }
    #[test]
    fn input_disappearing_before_switch_leaves_no_journal() {
        struct Vanished(std::cell::Cell<bool>);
        impl Defaults for Vanished {
            fn current(&self) -> Result<String, String> {
                if self.0.replace(true) {
                    Err("Unplugged".into())
                } else {
                    Ok("usb".into())
                }
            }
            fn target(&self) -> Result<String, String> {
                Ok("virtual".into())
            }
            fn set(&self, _: &str) -> Result<(), String> {
                panic!("No policy write")
            }
        }
        let path = std::env::temp_dir().join(format!(
            "dictto-unplug-before-switch-{}.json",
            std::process::id()
        ));
        assert_eq!(
            acquire(&path, &Vanished(false.into())).unwrap_err(),
            "Unplugged"
        );
        assert!(!path.exists());
    }
    #[test]
    fn unplugged_original_keeps_recovery_for_later() {
        struct Unplugged;
        impl Defaults for Unplugged {
            fn current(&self) -> Result<String, String> {
                Ok("virtual".into())
            }
            fn target(&self) -> Result<String, String> {
                Ok("virtual".into())
            }
            fn set(&self, _: &str) -> Result<(), String> {
                Err("Disconnected".into())
            }
        }
        let path =
            std::env::temp_dir().join(format!("dictto-recover-mic-{}.json", std::process::id()));
        let policy = Fake(RefCell::new("usb".to_string()));
        acquire(&path, &policy).unwrap();
        assert!(recover(&path, &Unplugged).is_err());
        assert!(path.exists());
        recover(&path, &policy).unwrap();
        assert!(!path.exists());
        assert_eq!(policy.current().unwrap(), "usb");
    }
    #[test]
    #[ignore = "read-only probe of the session's installed Windows audio policy interface"]
    fn windows_policy_interface_is_available_without_setting_any_default() {
        let _apartment = Apartment::open().unwrap();
        let _: Policy = unsafe { CoCreateInstance(&POLICY_CLASS, None, CLSCTX_ALL) }.unwrap();
    }
    #[test]
    fn restores_only_own_switch_including_after_restart() {
        let path =
            std::env::temp_dir().join(format!("dictto-route-test-{}.json", std::process::id()));
        let policy = Fake("usb".to_string().into());
        acquire(&path, &policy).unwrap();
        assert_eq!(policy.current().unwrap(), "virtual");
        recover(&path, &policy).unwrap();
        assert_eq!(policy.current().unwrap(), "usb");
        assert!(!path.exists());
        acquire(&path, &policy).unwrap();
        policy.set("user-chosen").unwrap();
        recover(&path, &policy).unwrap();
        assert_eq!(policy.current().unwrap(), "user-chosen");
        policy.set("virtual").unwrap();
        acquire(&path, &policy).unwrap();
        assert!(!path.exists());
    }
}
