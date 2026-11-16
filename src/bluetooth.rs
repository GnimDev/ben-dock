//! Bluetooth on this computer: whether there is a radio, whether it is on, and which
//! devices are paired with it.
//!
//! Every call here waits for Windows, so make them from a worker thread. That thread must
//! already be in a multi-threaded COM apartment; nothing here sets COM up.

use windows::core::{Interface, HSTRING};
use windows::Devices::Bluetooth::{BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
use windows::Foundation::IPropertyValue;

/// The property Windows keeps on a paired device to say whether it is connected.
const IS_CONNECTED: &str = "System.Devices.Aep.IsConnected";

/// A device paired with this computer.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub name: String,
    /// It is connected right now.
    pub connected: bool,
}

/// What there is to know about Bluetooth on this computer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bluetooth {
    /// The computer has a Bluetooth radio.
    pub available: bool,
    /// The radio is switched on.
    pub on: bool,
    /// Paired devices, connected ones first, then by name.
    pub devices: Vec<Device>,
}

/// Reads the radio's state and the paired devices. This waits for Windows (tens to
/// hundreds of milliseconds), so call it from a worker thread, never from the thread that animates.
pub fn read() -> Bluetooth {
    let Some(radio) = radio() else {
        return Bluetooth::default();
    };
    Bluetooth {
        available: true,
        on: radio.State().is_ok_and(|state| state == RadioState::On),
        devices: paired_devices(),
    }
}

/// Switches the Bluetooth radio on or off. Returns true if Windows did it.
/// This waits for Windows too, so call it from a worker thread.
pub fn set_on(on: bool) -> bool {
    switch(on) == Some(RadioAccessStatus::Allowed)
}

/// Asks Windows to switch the radio and passes on its answer. `None` means there is no
/// radio, or Windows did not answer.
fn switch(on: bool) -> Option<RadioAccessStatus> {
    // Windows wants an app to ask before it touches a radio.
    let access = Radio::RequestAccessAsync().ok()?.join().ok()?;
    if access != RadioAccessStatus::Allowed {
        return Some(access);
    }
    let state = if on { RadioState::On } else { RadioState::Off };
    radio()?.SetStateAsync(state).ok()?.join().ok()
}

/// Finds the Bluetooth radio, if the computer has one.
fn radio() -> Option<Radio> {
    let radios = Radio::GetRadiosAsync().ok()?.join().ok()?;
    let count = radios.Size().ok()?;
    (0..count)
        .filter_map(|index| radios.GetAt(index).ok())
        .find(|radio| radio.Kind().is_ok_and(|kind| kind == RadioKind::Bluetooth))
}

/// Asks Windows for the paired devices.
fn paired_devices() -> Vec<Device> {
    let property = HSTRING::from(IS_CONNECTED);

    // Windows keeps two lists, one for classic Bluetooth and one for Bluetooth Low Energy.
    // A device that speaks both is in both.
    let selectors = [
        BluetoothDevice::GetDeviceSelectorFromPairingState(true),
        BluetoothLEDevice::GetDeviceSelectorFromPairingState(true),
    ];

    // Start both searches before waiting for either, so they run side by side.
    let searches = selectors.map(|selector| {
        // What else to fetch for each device. `into()` builds the list the search takes, an
        // `IIterable<HSTRING>`. `windows` 0.62 does not re-export that type, so it is not
        // named here; passing `Some(&wanted)` is what lets the compiler work it out.
        let wanted = vec![property.clone()].into();
        DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(
            &selector.ok()?,
            Some(&wanted),
        )
        .ok()
    });

    let mut entries = Vec::new();
    for search in searches {
        let Some(found) = search.and_then(|search| search.join().ok()) else {
            continue;
        };
        // Counted by hand: `for info in &found` panics if Windows will not give an iterator.
        for index in 0..found.Size().unwrap_or(0) {
            let Ok(info) = found.GetAt(index) else {
                continue;
            };
            let name = info
                .Name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default();
            entries.push((name, is_connected(&info, &property)));
        }
    }
    tidy(entries)
}

/// Whether Windows says the device is connected right now. No answer counts as no.
fn is_connected(info: &DeviceInformation, property: &HSTRING) -> bool {
    info.Properties()
        .and_then(|properties| properties.Lookup(property))
        .and_then(|value| value.cast::<IPropertyValue>())
        .and_then(|value| value.GetBoolean())
        .unwrap_or(false)
}

/// Turns the entries Windows listed (a name, and whether it is connected) into the list to
/// show: one device per name, connected ones first, then by name. Entries without a name
/// are dropped.
fn tidy(entries: Vec<(String, bool)>) -> Vec<Device> {
    let mut devices: Vec<Device> = Vec::new();
    for (name, connected) in entries {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        // The same name twice is one device: it speaks both kinds of Bluetooth, or Windows
        // has it on record more than once.
        match devices.iter_mut().find(|device| device.name == name) {
            Some(device) => device.connected |= connected,
            None => devices.push(Device {
                name: name.to_owned(),
                connected,
            }),
        }
    }
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    devices
}
