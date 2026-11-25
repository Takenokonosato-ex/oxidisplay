//! Zero-leak minimal Direct3D 11 & DXGI bindings for IddCx swapchain surface readback.
//!
//! Uses RAII `ComPtr` wrappers for every COM interface so GPU resources and
//! surfaces are deterministically released on every frame and swapchain teardown.

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use core::ffi::c_void;
use core::mem::size_of;

use dl_protocol::DirtyRect;

use crate::wdf_iddcx::{succeeded, GUID, HRESULT, LUID};

pub const D3D11_SDK_VERSION: u32 = 7;
pub const D3D_DRIVER_TYPE_UNKNOWN: u32 = 0;
pub const D3D_DRIVER_TYPE_HARDWARE: u32 = 1;
pub const D3D11_CREATE_DEVICE_BGRA_SUPPORT: u32 = 0x20;
pub const D3D11_USAGE_STAGING: u32 = 3;
pub const D3D11_CPU_ACCESS_READ: u32 = 0x20000;
pub const D3D11_MAP_READ: u32 = 1;
pub const DXGI_FORMAT_B8G8R8A8_UNORM: u32 = 87;

pub const IID_IDXGIFACTORY1: GUID = GUID {
    data1: 0x770a_ae78,
    data2: 0xf26f,
    data3: 0x4dba,
    data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
};

pub const IID_IDXGIDEVICE: GUID = GUID {
    data1: 0x54ec_77fa,
    data2: 0x1377,
    data3: 0x44e6,
    data4: [0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c],
};

pub const IID_ID3D11TEXTURE2D: GUID = GUID {
    data1: 0x6f15_aaf2,
    data2: 0xd208,
    data3: 0x4e89,
    data4: [0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c],
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DXGI_SAMPLE_DESC {
    pub count: u32,
    pub quality: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct D3D11_TEXTURE2D_DESC {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    pub array_size: u32,
    pub format: u32,
    pub sample_desc: DXGI_SAMPLE_DESC,
    pub usage: u32,
    pub bind_flags: u32,
    pub cpu_access_flags: u32,
    pub misc_flags: u32,
}
const _: () = assert!(size_of::<D3D11_TEXTURE2D_DESC>() == 44);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct D3D11_MAPPED_SUBRESOURCE {
    pub data: *mut c_void,
    pub row_pitch: u32,
    pub depth_pitch: u32,
}
const _: () = assert!(size_of::<D3D11_MAPPED_SUBRESOURCE>() == 16);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct D3D11_BOX {
    pub left: u32,
    pub top: u32,
    pub front: u32,
    pub right: u32,
    pub bottom: u32,
    pub back: u32,
}
const _: () = assert!(size_of::<D3D11_BOX>() == 24);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct DXGI_ADAPTER_DESC1 {
    pub description: [u16; 128],
    pub vendor_id: u32,
    pub device_id: u32,
    pub sub_sys_id: u32,
    pub revision: u32,
    pub dedicated_video_memory: usize,
    pub dedicated_system_memory: usize,
    pub shared_system_memory: usize,
    pub adapter_luid: LUID,
    pub flags: u32,
    pub _pad: u32,
}
const _: () = assert!(size_of::<DXGI_ADAPTER_DESC1>() == 312);

extern "system" {
    fn CreateDXGIFactory1(riid: *const GUID, pp_factory: *mut *mut c_void) -> HRESULT;
    fn D3D11CreateDevice(
        p_adapter: *mut c_void,
        driver_type: u32,
        software: *mut c_void,
        flags: u32,
        p_feature_levels: *const u32,
        feature_levels: u32,
        sdk_version: u32,
        pp_device: *mut *mut c_void,
        p_feature_level: *mut u32,
        pp_immediate_context: *mut *mut c_void,
    ) -> HRESULT;
}

/// Deterministic RAII COM pointer. Guarantees `Release()` is always called on drop.
pub struct ComPtr {
    raw: *mut c_void,
}

unsafe impl Send for ComPtr {}

impl ComPtr {
    #[inline]
    pub unsafe fn from_raw(raw: *mut c_void) -> Option<Self> {
        if raw.is_null() {
            None
        } else {
            Some(Self { raw })
        }
    }

    #[inline]
    pub fn as_ptr(&self) -> *mut c_void {
        self.raw
    }

    #[inline]
    unsafe fn vtbl_slot(&self, slot: usize) -> *const c_void {
        let vtbl = *(self.raw as *const *const *const c_void);
        *vtbl.add(slot)
    }

    pub fn query_interface(&self, iid: &GUID) -> Result<ComPtr, HRESULT> {
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT =
                core::mem::transmute(self.vtbl_slot(0));
            let mut out = core::ptr::null_mut();
            let hr = f(self.raw, iid, &mut out);
            if succeeded(hr) {
                ComPtr::from_raw(out).ok_or(hr)
            } else {
                Err(hr)
            }
        }
    }
}

impl Drop for ComPtr {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe {
                let f: unsafe extern "system" fn(*mut c_void) -> u32 =
                    core::mem::transmute(self.vtbl_slot(2));
                f(self.raw);
            }
            self.raw = core::ptr::null_mut();
        }
    }
}

struct StagingSlot {
    tex: ComPtr,
    width: u32,
    height: u32,
    format: u32,
}

/// Holds a single D3D11 device/context on the DWM render adapter plus a double-buffered
/// ping-pong pair of pre-allocated staging textures for zero-stall GPU-to-CPU readback.
pub struct RenderDevice {
    device: ComPtr,
    context: ComPtr,
    dxgi_device: ComPtr,
    staging: [Option<StagingSlot>; 2],
}

impl RenderDevice {
    /// Create a D3D11 device on the DXGI adapter matching `render_adapter_luid`
    /// (falling back to the default hardware adapter if the LUID isn't found).
    pub fn new(render_adapter_luid: LUID) -> Result<Self, HRESULT> {
        let adapter = find_adapter_by_luid(render_adapter_luid).ok();

        let (adapter_ptr, driver_type) = match &adapter {
            Some(a) => (a.as_ptr(), D3D_DRIVER_TYPE_UNKNOWN),
            None => (core::ptr::null_mut(), D3D_DRIVER_TYPE_HARDWARE),
        };

        let mut dev_raw = core::ptr::null_mut();
        let mut ctx_raw = core::ptr::null_mut();
        let mut level = 0u32;
        let hr = unsafe {
            D3D11CreateDevice(
                adapter_ptr,
                driver_type,
                core::ptr::null_mut(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                core::ptr::null(),
                0,
                D3D11_SDK_VERSION,
                &mut dev_raw,
                &mut level,
                &mut ctx_raw,
            )
        };
        if !succeeded(hr) {
            return Err(hr);
        }

        let device = unsafe { ComPtr::from_raw(dev_raw) }.ok_or(hr)?;
        let context = unsafe { ComPtr::from_raw(ctx_raw) }.ok_or(hr)?;
        let dxgi_device = device.query_interface(&IID_IDXGIDEVICE)?;

        Ok(Self {
            device,
            context,
            dxgi_device,
            staging: [None, None],
        })
    }

    #[inline]
    pub fn dxgi_device_ptr(&self) -> *mut c_void {
        self.dxgi_device.as_ptr()
    }

    /// Ensure the reusable staging texture in `slot` (0 or 1) matches `(width, height, format)`.
    fn ensure_staging(&mut self, slot: usize, width: u32, height: u32, format: u32) -> Result<*mut c_void, HRESULT> {
        if let Some(ref s) = self.staging[slot] {
            if s.width == width && s.height == height && s.format == format {
                return Ok(s.tex.as_ptr());
            }
        }
        self.staging[slot] = None;

        let desc = D3D11_TEXTURE2D_DESC {
            width,
            height,
            mip_levels: 1,
            array_size: 1,
            format,
            sample_desc: DXGI_SAMPLE_DESC { count: 1, quality: 0 },
            usage: D3D11_USAGE_STAGING,
            bind_flags: 0,
            cpu_access_flags: D3D11_CPU_ACCESS_READ,
            misc_flags: 0,
        };
        let mut tex_raw = core::ptr::null_mut();
        let hr = unsafe {
            // ID3D11Device::CreateTexture2D = vtable slot 5
            let f: unsafe extern "system" fn(
                *mut c_void,
                *const D3D11_TEXTURE2D_DESC,
                *const c_void,
                *mut *mut c_void,
            ) -> HRESULT = core::mem::transmute(self.device.vtbl_slot(5));
            f(self.device.as_ptr(), &desc, core::ptr::null(), &mut tex_raw)
        };
        if !succeeded(hr) {
            return Err(hr);
        }
        let tex = unsafe { ComPtr::from_raw(tex_raw) }.ok_or(hr)?;
        let ptr = tex.as_ptr();
        self.staging[slot] = Some(StagingSlot {
            tex,
            width,
            height,
            format,
        });
        Ok(ptr)
    }

    /// Queue GPU copy of dirty rects (or full surface when `full_copy` is true) into staging `slot`,
    /// release our temporary QI ref on `src_dxgi_resource`, and immediately invoke `on_copy_queued`
    /// so `IddCxSwapChainFinishedProcessingFrame` unblocks DWM before any CPU `Map()` happens.
    pub fn copy_to_staging(
        &mut self,
        slot: usize,
        src_dxgi_resource: *mut c_void,
        dirty_rects: &[DirtyRect],
        full_copy: bool,
        on_copy_queued: impl FnOnce(),
    ) -> Result<(u32, u32), HRESULT> {
        if src_dxgi_resource.is_null() {
            on_copy_queued();
            return Err(-1);
        }

        let src_tex = unsafe {
            let vtbl = *(src_dxgi_resource as *const *const *const c_void);
            let qi: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT =
                core::mem::transmute(*vtbl.add(0));
            let mut out = core::ptr::null_mut();
            let hr = qi(src_dxgi_resource, &IID_ID3D11TEXTURE2D, &mut out);
            if !succeeded(hr) {
                on_copy_queued();
                return Err(hr);
            }
            match ComPtr::from_raw(out) {
                Some(p) => p,
                None => {
                    on_copy_queued();
                    return Err(hr);
                }
            }
        };

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            // ID3D11Texture2D::GetDesc = vtable slot 10
            let get_desc: unsafe extern "system" fn(*mut c_void, *mut D3D11_TEXTURE2D_DESC) =
                core::mem::transmute(src_tex.vtbl_slot(10));
            get_desc(src_tex.as_ptr(), &mut desc);
        }

        let staging_ptr = match self.ensure_staging(slot, desc.width, desc.height, desc.format) {
            Ok(p) => p,
            Err(hr) => {
                on_copy_queued();
                return Err(hr);
            }
        };

        unsafe {
            if full_copy || dirty_rects.is_empty() || dirty_rects.len() > 16 {
                // ID3D11DeviceContext::CopyResource = vtable slot 47
                let copy_res: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) =
                    core::mem::transmute(self.context.vtbl_slot(47));
                copy_res(self.context.as_ptr(), staging_ptr, src_tex.as_ptr());
            } else {
                // ID3D11DeviceContext::CopySubresourceRegion = vtable slot 46
                let copy_sub: unsafe extern "system" fn(
                    *mut c_void,
                    *mut c_void,
                    u32,
                    u32,
                    u32,
                    u32,
                    *mut c_void,
                    u32,
                    *const D3D11_BOX,
                ) = core::mem::transmute(self.context.vtbl_slot(46));

                for &r in dirty_rects {
                    if let Some(rc) = r.clamp(desc.width, desc.height) {
                        let bx = D3D11_BOX {
                            left: rc.left,
                            top: rc.top,
                            front: 0,
                            right: rc.right,
                            bottom: rc.bottom,
                            back: 1,
                        };
                        copy_sub(
                            self.context.as_ptr(),
                            staging_ptr,
                            0,
                            rc.left,
                            rc.top,
                            0,
                            src_tex.as_ptr(),
                            0,
                            &bx,
                        );
                    }
                }
            }
        }

        drop(src_tex);
        on_copy_queued();
        Ok((desc.width, desc.height))
    }

    /// Map a previously copied staging `slot` for CPU reading, invoke `f`, and unmap.
    pub fn map_staging<R>(
        &mut self,
        slot: usize,
        f: impl FnOnce(&[u8], usize, u32, u32) -> R,
    ) -> Result<R, HRESULT> {
        let Some(ref s) = self.staging[slot] else {
            return Err(-1);
        };
        let staging_ptr = s.tex.as_ptr();
        let width = s.width;
        let height = s.height;

        unsafe {
            // ID3D11DeviceContext::Map = vtable slot 14
            let map_fn: unsafe extern "system" fn(
                *mut c_void,
                *mut c_void,
                u32,
                u32,
                u32,
                *mut D3D11_MAPPED_SUBRESOURCE,
            ) -> HRESULT = core::mem::transmute(self.context.vtbl_slot(14));
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            let hr = map_fn(self.context.as_ptr(), staging_ptr, 0, D3D11_MAP_READ, 0, &mut mapped);
            if !succeeded(hr) {
                return Err(hr);
            }

            let total_bytes = (mapped.row_pitch as usize).saturating_mul(height as usize);
            let slice = core::slice::from_raw_parts(mapped.data as *const u8, total_bytes);
            let result = f(slice, mapped.row_pitch as usize, width, height);

            // ID3D11DeviceContext::Unmap = vtable slot 15
            let unmap_fn: unsafe extern "system" fn(*mut c_void, *mut c_void, u32) =
                core::mem::transmute(self.context.vtbl_slot(15));
            unmap_fn(self.context.as_ptr(), staging_ptr, 0);

            Ok(result)
        }
    }
}

fn find_adapter_by_luid(target: LUID) -> Result<ComPtr, HRESULT> {
    let mut factory_raw = core::ptr::null_mut();
    let hr = unsafe { CreateDXGIFactory1(&IID_IDXGIFACTORY1, &mut factory_raw) };
    if !succeeded(hr) {
        return Err(hr);
    }
    let factory = unsafe { ComPtr::from_raw(factory_raw) }.ok_or(hr)?;

    for idx in 0..32u32 {
        let mut adapter_raw = core::ptr::null_mut();
        let hr = unsafe {
            // IDXGIFactory1::EnumAdapters1 = vtable slot 12
            let enum_fn: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> HRESULT =
                core::mem::transmute(factory.vtbl_slot(12));
            enum_fn(factory.as_ptr(), idx, &mut adapter_raw)
        };
        if !succeeded(hr) {
            break;
        }
        if let Some(adapter) = unsafe { ComPtr::from_raw(adapter_raw) } {
            let mut desc: DXGI_ADAPTER_DESC1 = unsafe { core::mem::zeroed() };
            let dhr = unsafe {
                // IDXGIAdapter1::GetDesc1 = vtable slot 10
                let get_desc1: unsafe extern "system" fn(*mut c_void, *mut DXGI_ADAPTER_DESC1) -> HRESULT =
                    core::mem::transmute(adapter.vtbl_slot(10));
                get_desc1(adapter.as_ptr(), &mut desc)
            };
            if succeeded(dhr) && desc.adapter_luid == target {
                return Ok(adapter);
            }
        }
    }
    Err(-1)
}
