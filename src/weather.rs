//! The weather where this computer is: asked of Open-Meteo every twenty minutes, for the
//! place Windows says this is. Only that place goes out, rounded to a tenth of a degree
//! (some ten kilometres), and nothing else about the computer or its user.
//!
//! Windows may refuse to say where the computer is (Location is off, or this app is not
//! allowed it); then there is no weather, and the corner says how to turn it on. Like the
//! other watchers this runs on a thread of its own and nudges the app's window.

use crate::claude::number;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;
use windows::core::HSTRING;
use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator};
use windows::Foundation::Uri;
use windows::Web::Http::Filters::HttpBaseProtocolFilter;
use windows::Web::Http::HttpClient;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

const LOOK_EVERY: Duration = Duration::from_secs(20 * 60);

/// The weather now, and what today is expected to bring. Degrees Celsius, km/h, percent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sky {
    pub now: f32,
    pub feels: f32,
    pub humidity: f32,
    pub wind: f32,
    /// What the sky is doing, as the weather services number it (see `sign`).
    pub code: u32,
    pub day: bool,
    pub high: f32,
    pub low: f32,
    pub rain: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Report {
    Known(Sky),
    /// Windows does not say where the computer is.
    NoPlace,
    /// The weather service did not answer (no network, or an answer that made no sense).
    NoAnswer,
}

enum Wake {
    Look,
    Stop,
}

pub struct Weather {
    tx: Sender<Wake>,
    rx: Receiver<Report>,
}

impl Weather {
    /// Starts asking. `window` gets `message` whenever there is something new to `take`.
    pub fn start(window: HWND, message: u32) -> Weather {
        let (tx, wake) = channel::<Wake>();
        let (out, rx) = channel::<Report>();
        let window = window.0 as isize;
        let spawned = std::thread::Builder::new().name("weather".into()).spawn(move || watch(wake, out, window, message));
        if let Err(e) = spawned {
            crate::log!("could not start asking for the weather: {e}");
        }
        Weather { tx, rx }
    }

    /// Asks again now (the corner was opened).
    pub fn look(&self) {
        let _ = self.tx.send(Wake::Look);
    }

    pub fn take(&self) -> Option<Report> {
        self.rx.try_iter().last()
    }
}

impl Drop for Weather {
    fn drop(&mut self) {
        let _ = self.tx.send(Wake::Stop);
    }
}

/// A sign for what the sky is doing, and a word or two for it. The signs are ordinary
/// characters, written with the text (the font of symbols the app uses has none for weather).
pub fn sign(code: u32, day: bool) -> (&'static str, &'static str) {
    match code {
        0 => (if day { "☀" } else { "🌙" }, "Trời quang"),
        1 => (if day { "🌤" } else { "🌙" }, "Ít mây"),
        2 => ("⛅", "Có mây"),
        3 => ("☁", "Nhiều mây"),
        45 | 48 => ("🌫", "Sương mù"),
        51..=57 => ("🌦", "Mưa phùn"),
        61..=67 => ("🌧", "Mưa"),
        71..=77 | 85 | 86 => ("❄", "Tuyết"),
        80..=82 => ("🌧", "Mưa rào"),
        95..=99 => ("⛈", "Giông"),
        _ => ("☁", "Thời tiết"),
    }
}

fn watch(wake: Receiver<Wake>, out: Sender<Report>, window: isize, message: u32) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut last = None;
    loop {
        let report = read();
        if last != Some(report) {
            // Which place is asked about is not written down, only how it went.
            crate::log!("weather: {}", match report {
                Report::Known(_) => "read",
                Report::NoPlace => "Windows does not say where the computer is",
                Report::NoAnswer => "no answer from the weather service",
            });
            last = Some(report);
        }
        if out.send(report).is_err() {
            break;
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(window as *mut _)), message, WPARAM(0), LPARAM(0));
        }
        match wake.recv_timeout(LOOK_EVERY) {
            Ok(Wake::Look) | Err(RecvTimeoutError::Timeout) => {}
            Ok(Wake::Stop) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Where the computer is, as Windows knows it: latitude and longitude, rounded to a tenth
/// of a degree.
fn place() -> Option<(f64, f64)> {
    if Geolocator::RequestAccessAsync().ok()?.join().ok()? != GeolocationAccessStatus::Allowed {
        return None;
    }
    let at = Geolocator::new().ok()?.GetGeopositionAsync().ok()?.join().ok()?.Coordinate().ok()?.Point().ok()?.Position().ok()?;
    let tenth = |degrees: f64| (degrees * 10.0).round() / 10.0;
    Some((tenth(at.Latitude), tenth(at.Longitude)))
}

fn read() -> Report {
    let Some((latitude, longitude)) = place() else { return Report::NoPlace };
    let address = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={latitude:.1}&longitude={longitude:.1}&current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m,is_day&daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max&timezone=auto&forecast_days=1"
    );
    // Asked for as it is, not packed: Windows' own unpacking fails on this service's answers.
    let client = HttpBaseProtocolFilter::new().and_then(|filter| {
        filter.SetAutomaticDecompression(false)?;
        HttpClient::Create(&filter)
    });
    let answer = match client.and_then(|client| client.GetStringAsync(&Uri::CreateUri(&HSTRING::from(address))?)).and_then(|op| op.join()) {
        Ok(answer) => answer.to_string(),
        Err(e) => {
            crate::log!("weather: the service could not be asked: {e}");
            return Report::NoAnswer;
        }
    };
    match parse(&answer) {
        Some(sky) => Report::Known(sky),
        None => {
            // The answer itself names the place asked about, so only its size is noted.
            crate::log!("weather: an answer of {} characters that made no sense", answer.len());
            Report::NoAnswer
        }
    }
}

/// Picks the numbers out of the service's answer: what it is like now, and today's range.
fn parse(answer: &str) -> Option<Sky> {
    let now = &answer[answer.find("\"current\":{")?..];
    let today = &answer[answer.find("\"daily\":{")?..];
    let one = |key: &str| number(now, key).map(|value| value as f32);
    let first = |key: &str| number(today, key).map(|value| value as f32);
    Some(Sky {
        now: one("\"temperature_2m\":")?,
        feels: one("\"apparent_temperature\":")?,
        humidity: one("\"relative_humidity_2m\":")?,
        wind: one("\"wind_speed_10m\":")?,
        code: one("\"weather_code\":")? as u32,
        day: one("\"is_day\":")? != 0.0,
        high: first("\"temperature_2m_max\":[")?,
        low: first("\"temperature_2m_min\":[")?,
        rain: first("\"precipitation_probability_max\":[").unwrap_or(0.0),
    })
}
