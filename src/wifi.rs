//! Wi-Fi: the networks in range, which one is connected, and joining one.
//!
//! Everything goes through Windows' own Wi-Fi service (the WLAN API), the
//! same one Windows' network list uses. A password typed for a new network is
//! handed to that service as a saved network ("profile"), exactly as Windows
//! does when a network is joined from its own list; it is not kept anywhere
//! else and never written to the log.

use crate::util::wide;
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::NetworkManagement::WiFi::*;

const ERROR_ACCESS_DENIED: u32 = 5;

/// A Wi-Fi network in range.
#[derive(Clone, Debug)]
pub struct Network {
    pub name: String,
    /// The name's raw bytes, as broadcast.
    ssid: Vec<u8>,
    /// Signal, 0 to 100.
    pub signal: u32,
    /// Needs a password or a sign-in.
    pub secure: bool,
    /// Windows already has this network saved (it can be joined without asking anything).
    pub known: bool,
    pub connected: bool,
    /// The saved network's name in Windows, when `known`.
    profile: String,
    auth: i32,
    cipher: i32,
}

impl Network {
    /// Whether a password alone is enough to join (home networks). Company networks that ask
    /// for a user name and a certificate are not supported here.
    pub fn takes_password(&self) -> bool {
        self.auth == DOT11_AUTH_ALGO_RSNA_PSK.0 || self.auth == DOT11_AUTH_ALGO_WPA_PSK.0 || self.auth == DOT11_AUTH_ALGO_WPA3_SAE.0
    }
}

/// What looking for networks came back with.
#[derive(Clone, Debug)]
pub enum Scan {
    /// Strongest first; the connected one, if any, is first of all.
    Found(Vec<Network>),
    /// This computer has no Wi-Fi adapter (or the Wi-Fi service is not running).
    NoAdapter,
    /// Windows refused: since Windows 11 24H2 the list of networks is only given to apps when
    /// Location is switched on.
    Denied,
    Failed(u32),
}

/// An open session with the Wi-Fi service and the first adapter. Closed when dropped.
struct Session {
    handle: HANDLE,
    adapter: GUID,
}

impl Session {
    fn open() -> Result<Session, Scan> {
        unsafe {
            let (mut version, mut handle) = (0u32, HANDLE::default());
            let code = WlanOpenHandle(2, None, &mut version, &mut handle);
            if code != 0 {
                return Err(Scan::NoAdapter);
            }
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            let code = WlanEnumInterfaces(handle, None, &mut list);
            if code != 0 || list.is_null() {
                WlanCloseHandle(handle, None);
                return Err(if code == ERROR_ACCESS_DENIED { Scan::Denied } else { Scan::Failed(code) });
            }
            let adapter = ((*list).dwNumberOfItems > 0).then(|| (*list).InterfaceInfo[0].InterfaceGuid);
            WlanFreeMemory(list as *const _);
            match adapter {
                Some(adapter) => Ok(Session { handle, adapter }),
                None => {
                    WlanCloseHandle(handle, None);
                    Err(Scan::NoAdapter)
                }
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            WlanCloseHandle(self.handle, None);
        }
    }
}

/// The networks in range, one entry per name.
pub fn scan() -> Scan {
    let session = match Session::open() {
        Ok(s) => s,
        Err(why) => return why,
    };
    unsafe {
        let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
        let code = WlanGetAvailableNetworkList(session.handle, &session.adapter, 0, None, &mut list);
        if code != 0 || list.is_null() {
            return if code == ERROR_ACCESS_DENIED { Scan::Denied } else { Scan::Failed(code) };
        }
        let count = (*list).dwNumberOfItems as usize;
        let items = std::slice::from_raw_parts((*list).Network.as_ptr(), count);
        let mut found: Vec<Network> = Vec::new();
        for item in items {
            let length = (item.dot11Ssid.uSSIDLength as usize).min(32);
            let ssid = item.dot11Ssid.ucSSID[..length].to_vec();
            if ssid.is_empty() {
                // A hidden network: it has no name to show.
                continue;
            }
            let profile_len = item.strProfileName.iter().position(|&c| c == 0).unwrap_or(item.strProfileName.len());
            let network = Network {
                name: String::from_utf8_lossy(&ssid).into_owned(),
                ssid,
                signal: item.wlanSignalQuality.min(100),
                secure: item.bSecurityEnabled.as_bool(),
                known: item.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE != 0,
                connected: item.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
                profile: String::from_utf16_lossy(&item.strProfileName[..profile_len]),
                auth: item.dot11DefaultAuthAlgorithm.0,
                cipher: item.dot11DefaultCipherAlgorithm.0,
            };
            // Windows lists a network once per saved copy and once bare; keep the most useful.
            match found.iter_mut().find(|n| n.ssid == network.ssid) {
                Some(seen) => {
                    if (network.connected, network.known, network.signal) > (seen.connected, seen.known, seen.signal) {
                        *seen = network;
                    }
                }
                None => found.push(network),
            }
        }
        WlanFreeMemory(list as *const _);
        found.sort_by_key(|n| std::cmp::Reverse((n.connected, n.signal)));
        Scan::Found(found)
    }
}

/// Asks the adapter to look for networks again. The fresh list is ready a few seconds later.
pub fn refresh() {
    if let Ok(session) = Session::open() {
        unsafe {
            WlanScan(session.handle, &session.adapter, None, None, None);
        }
    }
}

/// Starts joining a network Windows already has saved. `Err` holds Windows' error code.
/// Success only means the attempt has started; whether it worked shows in a later `scan`.
pub fn join_saved(network: &Network) -> Result<(), u32> {
    let session = Session::open().map_err(|_| 1u32)?;
    connect(&session, &network.profile)
}

/// Starts joining a new network: saves it in Windows with this password (none for an open
/// network), then connects. `Err` holds Windows' error code, or its reason code for a
/// network description it rejected.
pub fn join_new(network: &Network, password: &str) -> Result<(), u32> {
    let session = Session::open().map_err(|_| 1u32)?;
    let xml = wide(&profile_xml(&network.name, &network.ssid, network.secure, network.auth, network.cipher, password));
    unsafe {
        let mut reason = 0u32;
        let code = WlanSetProfile(session.handle, &session.adapter, 0, PCWSTR(xml.as_ptr()), PCWSTR::null(), true, None, &mut reason);
        if code != 0 {
            return Err(if reason != 0 { reason } else { code });
        }
    }
    connect(&session, &network.name)
}

/// Removes a saved network (one that was just saved with a password that did not work).
pub fn forget(name: &str) {
    if let Ok(session) = Session::open() {
        let name = wide(name);
        unsafe {
            WlanDeleteProfile(session.handle, &session.adapter, PCWSTR(name.as_ptr()), None);
        }
    }
}

fn connect(session: &Session, profile: &str) -> Result<(), u32> {
    let profile = wide(profile);
    let parameters = WLAN_CONNECTION_PARAMETERS {
        wlanConnectionMode: wlan_connection_mode_profile,
        strProfile: PCWSTR(profile.as_ptr()),
        pDot11Ssid: std::ptr::null_mut(),
        pDesiredBssidList: std::ptr::null_mut(),
        dot11BssType: dot11_BSS_type_infrastructure,
        dwFlags: 0,
    };
    match unsafe { WlanConnect(session.handle, &session.adapter, &parameters, None) } {
        0 => Ok(()),
        code => Err(code),
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// The description of a network in the form Windows saves networks in.
fn profile_xml(name: &str, ssid: &[u8], secure: bool, auth: i32, cipher: i32, password: &str) -> String {
    let hex: String = ssid.iter().map(|b| format!("{b:02X}")).collect();
    let name = escape(name);
    let security = if !secure {
        "<authEncryption><authentication>open</authentication><encryption>none</encryption><useOneX>false</useOneX></authEncryption>".to_string()
    } else {
        let authentication = if auth == DOT11_AUTH_ALGO_WPA3_SAE.0 {
            "WPA3SAE"
        } else if auth == DOT11_AUTH_ALGO_WPA_PSK.0 {
            "WPAPSK"
        } else {
            "WPA2PSK"
        };
        let encryption = if cipher == DOT11_CIPHER_ALGO_TKIP.0 { "TKIP" } else { "AES" };
        format!(
            "<authEncryption><authentication>{authentication}</authentication><encryption>{encryption}</encryption><useOneX>false</useOneX></authEncryption>\
             <sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{}</keyMaterial></sharedKey>",
            escape(password)
        )
    };
    format!(
        "<?xml version=\"1.0\"?><WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\"><name>{name}</name>\
         <SSIDConfig><SSID><hex>{hex}</hex><name>{name}</name></SSID></SSIDConfig><connectionType>ESS</connectionType>\
         <connectionMode>auto</connectionMode><MSM><security>{security}</security></MSM></WLANProfile>"
    )
}
