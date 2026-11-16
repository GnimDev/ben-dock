//! Playback devices: list them, and choose the one Windows plays through.
//!
//! The caller must already have COM initialised on the calling thread.
//! Nothing in this file starts or stops COM.

use std::ffi::c_void;

use windows::core::{IUnknown, Interface, GUID, HRESULT, PCWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    eCommunications, eConsole, eMultimedia, eRender, ERole, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};
use windows::Win32::System::Variant::VT_LPWSTR;

/// A playback device (speakers, headphones, a monitor's audio...).
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    /// Windows' identity for the device (the endpoint id string).
    pub id: String,
    /// The name shown to people, e.g. "Speakers (Realtek(R) Audio)".
    pub name: String,
    /// Windows plays through this one.
    pub is_default: bool,
}

/// The playback devices that are plugged in and enabled, the default one flagged.
///
/// The list is empty if Windows cannot be asked. A device whose id or name
/// cannot be read (it was unplugged meanwhile, say) is left out.
pub fn outputs() -> Vec<Output> {
    list_outputs().unwrap_or_default()
}

/// Makes a device the default one for everything (console, multimedia and communications roles).
/// Returns false if Windows refused or the call is unavailable.
///
/// `id` must be the id of one of the current [`outputs`]. Anything else returns false.
///
/// Windows has no documented way to do this. This goes through `IPolicyConfig`,
/// an undocumented Windows interface (the one EarTrumpet and SoundSwitch use),
/// so it may stop working in a future Windows.
pub fn set_default_output(id: &str) -> bool {
    // Only ever hand Windows the id of a playback device it has just listed.
    if !outputs().iter().any(|output| output.id == id) {
        return false;
    }
    let Some(policy) = policy_config() else {
        return false;
    };
    let wide = to_wide(id);

    // SAFETY: `policy` is a live `IPolicyConfig` pointer, so it points at a
    // pointer to that interface's method table. `wide` is NUL-terminated and
    // outlives the calls.
    unsafe {
        let this = policy.as_raw();
        let table = *(this as *const *const IPolicyConfigVtbl);
        let set_default_endpoint = (*table).set_default_endpoint;

        // Try all three roles even if one fails, and report whether all were set.
        let mut all_set = true;
        for role in [eConsole, eMultimedia, eCommunications] {
            all_set &= set_default_endpoint(this, PCWSTR(wide.as_ptr()), role).is_ok();
        }
        all_set
    }
}

/// How loud what Windows is playing is, right now: the needle of the default playback
/// device's own level meter. Nothing is recorded or listened to; Windows keeps this number
/// for every device and hands it over when asked.
pub struct Meter {
    meter: IAudioMeterInformation,
}

impl Meter {
    /// The meter of the device Windows plays through at the moment. `None` if there is no
    /// such device, or Windows cannot be asked.
    pub fn new() -> Option<Self> {
        // SAFETY: ordinary COM calls on interfaces this function owns.
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
            let meter = device.Activate::<IAudioMeterInformation>(CLSCTX_ALL, None).ok()?;
            Some(Meter { meter })
        }
    }

    /// The loudest the sound has been in the last few hundredths of a second, from 0
    /// (silence) to 1. `None` if the device is gone (unplugged, or no longer the default).
    pub fn peak(&self) -> Option<f32> {
        // SAFETY: an ordinary COM call on an interface this struct owns.
        unsafe { self.meter.GetPeakValue().ok() }
    }
}

/// Asks Windows for the active playback devices.
fn list_outputs() -> windows::core::Result<Vec<Output>> {
    // SAFETY: ordinary COM calls on interfaces this function owns.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;

        // With no playback device at all there is no default one. That is not an error.
        let default_id = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .ok()
            .and_then(|device| device_id(&device));

        let mut list = Vec::new();
        for index in 0..devices.GetCount()? {
            let Ok(device) = devices.Item(index) else {
                continue;
            };
            let (Some(id), Some(name)) = (device_id(&device), device_name(&device)) else {
                continue;
            };
            let is_default = default_id.as_deref() == Some(id.as_str());
            list.push(Output { id, name, is_default });
        }
        Ok(list)
    }
}

/// Reads the id Windows gives a device.
fn device_id(device: &IMMDevice) -> Option<String> {
    // SAFETY: `GetId` returns a NUL-terminated string that the caller owns and
    // must free with `CoTaskMemFree`. It is copied before it is freed.
    unsafe {
        let text = device.GetId().ok()?;
        if text.is_null() {
            return None;
        }
        let id = String::from_utf16(text.as_wide()).ok();
        CoTaskMemFree(Some(text.as_ptr() as *const c_void));
        id.filter(|id| !id.is_empty())
    }
}

/// Reads the name Windows shows for a device.
fn device_name(device: &IMMDevice) -> Option<String> {
    // SAFETY: the string field is only read when the value says it holds a
    // string. The value frees that string when dropped, after it is copied.
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ).ok()?;
        let value = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        if value.vt() != VT_LPWSTR {
            return None;
        }
        let text = value.Anonymous.Anonymous.Anonymous.pwszVal;
        if text.is_null() {
            return None;
        }
        let name = String::from_utf16_lossy(text.as_wide());
        (!name.is_empty()).then_some(name)
    }
}

/// Turns text into the NUL-terminated UTF-16 that Windows expects.
fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Class id of `CPolicyConfigClient`, the Windows object that implements `IPolicyConfig`.
const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// Interface id of `IPolicyConfig`.
const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);

/// The method table of `IPolicyConfig`, in the order Windows uses.
///
/// The ids above and this order are the ones EarTrumpet and SoundSwitch use.
/// Every method this file does not call is a pointer-sized placeholder, so it
/// keeps its place in the table but cannot be called by mistake.
#[repr(C)]
struct IPolicyConfigVtbl {
    // IUnknown
    query_interface: usize,
    add_ref: usize,
    release: usize,
    // IPolicyConfig
    get_mix_format: usize,
    get_device_format: usize,
    reset_device_format: usize,
    set_device_format: usize,
    get_processing_period: usize,
    set_processing_period: usize,
    get_share_mode: usize,
    set_share_mode: usize,
    get_property_value: usize,
    set_property_value: usize,
    /// `HRESULT SetDefaultEndpoint(PCWSTR deviceId, ERole role)`
    set_default_endpoint:
        unsafe extern "system" fn(this: *mut c_void, device_id: PCWSTR, role: ERole) -> HRESULT,
    set_endpoint_visibility: usize,
}

// `SetDefaultEndpoint` is the 11th method after the three of `IUnknown`.
const _: () = assert!(
    std::mem::offset_of!(IPolicyConfigVtbl, set_default_endpoint)
        == 13 * std::mem::size_of::<usize>()
);

/// Creates Windows' policy object and returns its `IPolicyConfig` interface.
///
/// The interface is held as an `IUnknown`, which every COM interface also is,
/// so it is released when dropped.
fn policy_config() -> Option<IUnknown> {
    // SAFETY: `raw` is a valid place for `query` to write an interface
    // pointer. On success that pointer is ours to release, which the returned
    // `IUnknown` does.
    unsafe {
        let client: IUnknown =
            CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL).ok()?;
        let mut raw = std::ptr::null_mut();
        if client.query(&IID_POLICY_CONFIG, &mut raw).is_err() || raw.is_null() {
            return None;
        }
        Some(IUnknown::from_raw(raw))
    }
}
