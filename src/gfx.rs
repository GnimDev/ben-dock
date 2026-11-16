//! Graphics devices. The app does not draw frames itself: it hands Windows a
//! tree of visuals (see `scene.rs`) and only uses Direct2D to paint an icon or
//! a piece of text once, when it first appears.
//!
//! That is why the device is, by default, a software one. Windows' compositor
//! still puts everything on screen with the graphics card; but a hardware
//! device in *this* process loads the card vendor's driver into it, which on
//! its own takes more memory than the rest of the app together, to paint a
//! few dozen small pictures. The software device paints those on the
//! processor instead and leaves the driver out.

use windows::core::{Interface, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::*;

pub struct Gfx {
    d3d: ID3D11Device,
    pub d2d: ID2D1Device,
    pub dwrite: IDWriteFactory,
}

impl Gfx {
    /// `hardware`: paint with the graphics card (uses more memory; see above).
    pub fn new(hardware: bool) -> Result<Self> {
        unsafe {
            let (d3d, software) = create_d3d_device(hardware)?;
            let dxgi_device: IDXGIDevice = d3d.cast()?;
            if let Ok(desc) = dxgi_device.GetAdapter().and_then(|a| a.GetDesc()) {
                crate::log!("painting with: {}{}", crate::util::from_wide(&desc.Description), if software { " (on the processor)" } else { "" });
            }
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d = factory.CreateDevice(&dxgi_device)?;
            // Direct2D keeps painted pieces around in case they are needed again. Everything
            // here is painted once and then owned by the compositor, so a small store is enough.
            d2d.SetMaximumTextureMemory(8 * 1024 * 1024);
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            Ok(Gfx { d3d, d2d, dwrite })
        }
    }

    /// True after the GPU device was reset (driver update, sleep, remote desktop).
    /// Everything painted with it has to be painted again on a new device.
    pub fn lost(&self) -> bool {
        unsafe { self.d3d.GetDeviceRemovedReason().is_err() }
    }

    /// Gives back what the devices keep ready for more painting. Called when the app has
    /// been at rest for a while; the next thing painted simply takes a moment longer.
    pub fn trim(&self) {
        unsafe {
            self.d2d.ClearResources(0);
            if let Ok(context) = self.d3d.GetImmediateContext() {
                context.ClearState();
                context.Flush();
            }
            if let Ok(device) = self.d3d.cast::<IDXGIDevice3>() {
                device.Trim();
            }
        }
    }
}

unsafe fn create_d3d_device(hardware: bool) -> Result<(ID3D11Device, bool)> {
    let make = |adapter: Option<&IDXGIAdapter>, driver: D3D_DRIVER_TYPE, flags: D3D11_CREATE_DEVICE_FLAG| -> Result<ID3D11Device> {
        let mut device = None;
        unsafe { D3D11CreateDevice(adapter, driver, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut device), None, None)? };
        device.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))
    };
    let base = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
    // The software device paints on the calling thread and starts no threads of its own.
    let software = || make(None, D3D_DRIVER_TYPE_WARP, base | D3D11_CREATE_DEVICE_PREVENT_INTERNAL_THREADING_OPTIMIZATIONS);
    let on_card = || -> Result<ID3D11Device> {
        // On a laptop with two graphics cards, the one that uses the least power is plenty.
        let frugal = unsafe { CreateDXGIFactory1::<IDXGIFactory6>() }.and_then(|f| unsafe { f.EnumAdapterByGpuPreference::<IDXGIAdapter>(0, DXGI_GPU_PREFERENCE_MINIMUM_POWER) });
        match frugal {
            Ok(adapter) => make(Some(&adapter), D3D_DRIVER_TYPE_UNKNOWN, base).or_else(|_| make(None, D3D_DRIVER_TYPE_HARDWARE, base)),
            Err(_) => make(None, D3D_DRIVER_TYPE_HARDWARE, base),
        }
    };
    let attempts: [(&dyn Fn() -> Result<ID3D11Device>, bool); 2] = if hardware { [(&on_card, false), (&software, true)] } else { [(&software, true), (&on_card, false)] };
    let mut last = None;
    for (attempt, is_software) in attempts {
        match attempt() {
            Ok(device) => return Ok((device, is_software)),
            Err(e) => {
                crate::log!("could not create a {} graphics device: {e}", if is_software { "software" } else { "hardware" });
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL)))
}

pub fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

pub fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}
