use dictto_core::speech::route::Defaults;
use windows::core::{IUnknown_Vtbl, Interface, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Devices::{DeviceAndDriverInstallation::*, Properties::*};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "read-only probe of the session's installed Windows audio policy interface"]
    fn windows_policy_interface_is_available_without_setting_any_default() {
        let _apartment = Apartment::open().unwrap();
        let _: Policy = unsafe { CoCreateInstance(&POLICY_CLASS, None, CLSCTX_ALL) }.unwrap();
    }
}
