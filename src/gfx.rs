//! GPU devices. The dock does not draw frames itself: it hands Windows a tree
//! of visuals (see `glass.rs`) and only uses Direct2D to paint an icon or a
//! piece of text once, when it first appears.

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
    pub fn new() -> Result<Self> {
        unsafe {
            let d3d = create_d3d_device()?;
            let dxgi_device: IDXGIDevice = d3d.cast()?;
            if let Ok(desc) = dxgi_device.GetAdapter().and_then(|a| a.GetDesc()) {
                crate::log!("GPU: {}", crate::util::from_wide(&desc.Description));
            }
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d = factory.CreateDevice(&dxgi_device)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            Ok(Gfx { d3d, d2d, dwrite })
        }
    }

    /// True after the GPU device was reset (driver update, sleep, remote desktop).
    /// Everything painted with it has to be painted again on a new device.
    pub fn lost(&self) -> bool {
        unsafe { self.d3d.GetDeviceRemovedReason().is_err() }
    }
}

unsafe fn create_d3d_device() -> Result<ID3D11Device> {
    let mut last = None;
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let mut device = None;
        let r = unsafe {
            D3D11CreateDevice(None, driver, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, None)
        };
        match (r, device) {
            (Ok(()), Some(d)) => return Ok(d),
            (Err(e), _) => {
                crate::log!("D3D11CreateDevice({:?}) failed: {e}", driver);
                last = Some(e);
            }
            _ => {}
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
