//! Native Windows UMDF 2.33 + IddCx 1.10 Indirect Display Driver for DisplayLink DL-1x5,
//! written from scratch in Rust.
//!
//! Design goals:
//! 1. Zero memory leaks: deterministic RAII ownership of every WDF/COM/thread/buffer resource.
//! 2. Smooth 60 FPS on USB 2.0: 8-slot async Bulk OUT ring, multi-span shadow-buffer diffing,
//!    hardware `WRITECOPY16` move regions, dirty-rect-only GPU staging copy, MMCSS priority.
//! 3. Clean recovery on power transitions (D0/D3) and surprise USB removal.

mod d3d;
mod swapchain;
mod usb;
mod wdf_iddcx;

use core::ffi::c_void;
use core::mem::size_of;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use dl_protocol::{
    parse_edid, BlankMode, VideoMode, DTU_1031_FALLBACK_EDID, EDID_BLOCK_SIZE,
};

use crate::swapchain::SwapChainProcessor;
use crate::usb::UsbTransport;
use crate::wdf_iddcx::*;

/// Stable container GUID for our virtualized DisplayLink monitor endpoint.
const MONITOR_CONTAINER_GUID: GUID = GUID {
    data1: 0x5D1A_7E90,
    data2: 0x04A3,
    data3: 0x4165,
    data4: [0x92, 0xD1, 0x52, 0x55, 0x53, 0x54, 0x44, 0x4C],
};

// Static UTF-16 null-terminated endpoint diagnostic strings (required non-empty by IddCx).
static ENDPOINT_FRIENDLY: &[u16] = &[
    b'D' as u16, b'i' as u16, b's' as u16, b'p' as u16, b'l' as u16, b'a' as u16, b'y' as u16,
    b'L' as u16, b'i' as u16, b'n' as u16, b'k' as u16, b' ' as u16, b'R' as u16, b'u' as u16,
    b's' as u16, b't' as u16, 0,
];
static ENDPOINT_MODEL: &[u16] = &[
    b'D' as u16, b'L' as u16, b'-' as u16, b'1' as u16, b'x' as u16, b'5' as u16, b' ' as u16,
    b'D' as u16, b'T' as u16, b'U' as u16, b'-' as u16, b'1' as u16, b'0' as u16, b'3' as u16,
    b'1' as u16, 0,
];
static ENDPOINT_MFG: &[u16] = &[
    b'R' as u16, b'u' as u16, b's' as u16, b't' as u16, b' ' as u16, b'D' as u16, b'i' as u16,
    b's' as u16, b'p' as u16, b'l' as u16, b'a' as u16, b'y' as u16, b'L' as u16, b'i' as u16,
    b'n' as u16, b'k' as u16, 0,
];

/// Per-WDFDEVICE state, stored safely in our global registry keyed by WDF handles.
struct DeviceContext {
    _wdf_device: WDFDEVICE,
    adapter: Option<IDDCX_ADAPTER>,
    monitor: Option<IDDCX_MONITOR>,
    adapter_initialized: bool,
    edid: [u8; EDID_BLOCK_SIZE],
    supported_modes: Vec<VideoMode>,
    active_mode: VideoMode,
    usb: Arc<Mutex<Option<UsbTransport>>>,
    swapchain_proc: Option<SwapChainProcessor>,
}

unsafe impl Send for DeviceContext {}

#[derive(Default)]
struct DriverRegistry {
    by_device: HashMap<usize, Arc<Mutex<DeviceContext>>>,
    by_adapter: HashMap<usize, Arc<Mutex<DeviceContext>>>,
    by_monitor: HashMap<usize, Arc<Mutex<DeviceContext>>>,
}

static REGISTRY: OnceLock<Mutex<DriverRegistry>> = OnceLock::new();

extern "system" {
    fn OutputDebugStringA(lp_output_string: *const u8);
}

/// Lightweight diagnostic trace.
/// Always emits to Windows debug output (`OutputDebugStringA`, zero disk I/O).
/// Only appends to `C:\Users\Public\dl_rust_idd.log` when the opt-in trigger file
/// `C:\Users\Public\dl_rust_idd.debug` exists.
pub(crate) fn trace_log(msg: &str) {
    let line = format!("[dl_rust_idd] {msg}\n");
    let mut nul = Vec::with_capacity(line.len() + 1);
    nul.extend_from_slice(line.as_bytes());
    nul.push(0);
    unsafe { OutputDebugStringA(nul.as_ptr()) };

    if std::path::Path::new(r"C:\Users\Public\dl_rust_idd.debug").exists() {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(r"C:\Users\Public\dl_rust_idd.log")
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

fn registry() -> &'static Mutex<DriverRegistry> {
    REGISTRY.get_or_init(|| Mutex::new(DriverRegistry::default()))
}

fn fill_video_signal_info(mode: &VideoMode, is_target_mode: bool) -> DISPLAYCONFIG_VIDEO_SIGNAL_INFO {
    // CRITICAL: Report a small-numerator reduced integer refresh rate (e.g. 60/1) to IddCx!
    // Windows `IndirectKmd.sys` / `dxgkrnl.sys` multiplies 64-bit `QPC * vSyncFreq.Numerator`
    // internally when computing `FramePresentQpcTime`. Passing a raw pixel-clock numerator like
    // `71_110_000 / 1_185_120` overflows signed/unsigned 64-bit QPC after just 3.6–7.2 hours
    // of system uptime, jumping `FramePresentQpcTime` years into the future and locking DWM
    // onto its 2,000 ms (0.5 FPS) fallback timer! With `60 / 1`, 64-bit QPC lasts 487 years.
    // (The actual DL-1x5 hardware modeset still programs the exact EDID 71.11 MHz timing.)
    let v_hz = ((mode.refresh_mhz() + 500) / 1000).max(1);
    let h_hz = v_hz.saturating_mul(mode.vtotal as u32).max(1);
    let pixel_rate = (h_hz as u64).saturating_mul(mode.htotal as u64);

    // IddCx rule:
    // - videoStandard (bits 0..16) = D3DKMDT_VSS_OTHER (255)
    // - vSyncFreqDivider (bits 16..22):
    //     * IDDCX_MONITOR_MODE requires 0
    //     * IDDCX_TARGET_MODE requires >= 1 (1 = full 60Hz desktop update rate)
    let additional_signal_info = if is_target_mode {
        (1u32 << 16) | D3DKMDT_VSS_OTHER
    } else {
        D3DKMDT_VSS_OTHER
    };

    DISPLAYCONFIG_VIDEO_SIGNAL_INFO {
        pixel_rate,
        h_sync_freq: DISPLAYCONFIG_RATIONAL {
            numerator: h_hz,
            denominator: 1,
        },
        v_sync_freq: DISPLAYCONFIG_RATIONAL {
            numerator: v_hz,
            denominator: 1,
        },
        active_size: DISPLAYCONFIG_2DREGION {
            cx: mode.hdisplay as u32,
            cy: mode.vdisplay as u32,
        },
        total_size: DISPLAYCONFIG_2DREGION {
            cx: mode.htotal as u32,
            cy: mode.vtotal as u32,
        },
        additional_signal_info,
        scan_line_ordering: DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE,
    }
}

fn build_modes_from_edid(edid: &[u8]) -> Vec<VideoMode> {
    let mut modes = Vec::new();
    if let Some(parsed) = parse_edid(edid) {
        for m in parsed.modes {
            if m.pixel_count() <= 2_080_000 {
                modes.push(m);
            }
        }
    }
    if modes.is_empty() {
        modes.push(VideoMode::DTU_1031_1280X800_60);
    }
    modes
}

// =============================================================================
// UMDF 2.33 Entry Point & PnP/Power Callbacks
// =============================================================================

/// Entry point invoked by `FxDriverEntryUm` (in `WdfDriverStubUm.lib`).
///
/// # Safety
/// Called exclusively by the Windows UMDF 2.33 loader (`WUDFHost.exe`) with valid
/// `DRIVER_OBJECT` and `UNICODE_STRING` pointers.
#[no_mangle]
pub unsafe extern "system" fn DriverEntry(
    driver_object: *mut c_void,
    registry_path: *const UNICODE_STRING,
) -> NTSTATUS {
    let mut config = WDF_DRIVER_CONFIG {
        size: size_of::<WDF_DRIVER_CONFIG>() as u32,
        _pad0: 0,
        evt_driver_device_add: Some(evt_driver_device_add),
        evt_driver_unload: Some(evt_driver_unload),
        driver_init_flags: 0,
        driver_pool_tag: u32::from_le_bytes(*b"DLRu"),
    };

    let mut driver: WDFDRIVER = core::ptr::null_mut();
    let st = wdf_driver_create(
        driver_object,
        registry_path,
        core::ptr::null_mut(),
        &mut config,
        &mut driver,
    );
    trace_log(&format!("DriverEntry: status=0x{:08X}", st as u32));
    st
}

unsafe extern "system" fn evt_driver_unload(_driver: WDFDRIVER) {
    if let Ok(mut reg) = registry().lock() {
        reg.by_monitor.clear();
        reg.by_adapter.clear();
        reg.by_device.clear();
    }
}

unsafe extern "system" fn evt_driver_device_add(
    _driver: WDFDRIVER,
    mut device_init: PWDFDEVICE_INIT,
) -> NTSTATUS {
    // 1. Register WDF PnP/Power callbacks.
    let mut pnp = WDF_PNPPOWER_EVENT_CALLBACKS {
        evt_device_prepare_hardware: Some(evt_device_prepare_hardware),
        evt_device_release_hardware: Some(evt_device_release_hardware),
        evt_device_d0_entry: Some(evt_device_d0_entry),
        evt_device_d0_exit: Some(evt_device_d0_exit),
        evt_device_surprise_removal: Some(evt_device_surprise_removal),
        ..Default::default()
    };
    wdf_device_init_set_pnppower_callbacks(device_init, &mut pnp);

    // 2. Register IddCx 1.10 client callbacks before WdfDeviceCreate.
    let idd_cfg = IDD_CX_CLIENT_CONFIG {
        evt_idd_cx_adapter_init_finished: Some(evt_idd_cx_adapter_init_finished),
        evt_idd_cx_adapter_commit_modes: Some(evt_idd_cx_adapter_commit_modes),
        evt_idd_cx_parse_monitor_description: Some(evt_idd_cx_parse_monitor_description),
        evt_idd_cx_monitor_get_default_description_modes: Some(
            evt_idd_cx_monitor_get_default_description_modes,
        ),
        evt_idd_cx_monitor_query_target_modes: Some(evt_idd_cx_monitor_query_target_modes),
        evt_idd_cx_monitor_assign_swap_chain: Some(evt_idd_cx_monitor_assign_swap_chain),
        evt_idd_cx_monitor_unassign_swap_chain: Some(evt_idd_cx_monitor_unassign_swap_chain),
        ..Default::default()
    };

    let status = idd_cx_device_init_config(device_init, &idd_cfg);
    trace_log(&format!("EvtDriverDeviceAdd: IddCxDeviceInitConfig=0x{:08X}", status as u32));
    if !nt_success(status) {
        return status;
    }

    // 3. Create WDFDEVICE with cleanup callback so our Rust context is always freed.
    let mut attrs = WDF_OBJECT_ATTRIBUTES::init(core::ptr::null_mut());
    attrs.evt_cleanup_callback = Some(evt_device_cleanup);

    let mut device: WDFDEVICE = core::ptr::null_mut();
    let status = wdf_device_create(&mut device_init, &mut attrs, &mut device);
    trace_log(&format!("EvtDriverDeviceAdd: WdfDeviceCreate=0x{:08X}", status as u32));
    if !nt_success(status) {
        return status;
    }

    // 4. Initialize IddCx on the created WDFDEVICE.
    let status = idd_cx_device_initialize(device);
    trace_log(&format!("EvtDriverDeviceAdd: IddCxDeviceInitialize=0x{:08X}", status as u32));
    if !nt_success(status) {
        return status;
    }

    let ctx = Arc::new(Mutex::new(DeviceContext {
        _wdf_device: device,
        adapter: None,
        monitor: None,
        adapter_initialized: false,
        edid: DTU_1031_FALLBACK_EDID,
        supported_modes: vec![VideoMode::DTU_1031_1280X800_60],
        active_mode: VideoMode::DTU_1031_1280X800_60,
        usb: Arc::new(Mutex::new(None)),
        swapchain_proc: None,
    }));

    registry().lock().unwrap().by_device.insert(device as usize, ctx);
    STATUS_SUCCESS
}

unsafe extern "system" fn evt_device_cleanup(obj: WDFOBJECT) {
    let key = obj as usize;
    let ctx = registry().lock().unwrap().by_device.remove(&key);
    if let Some(ctx) = ctx {
        let mut c = ctx.lock().unwrap();
        if let Some(proc) = c.swapchain_proc.take() {
            proc.stop();
        }
        if let Ok(mut usb_guard) = c.usb.lock() {
            *usb_guard = None;
        }
        let mut reg = registry().lock().unwrap();
        if let Some(m) = c.monitor.take() {
            reg.by_monitor.remove(&(m as usize));
        }
        if let Some(a) = c.adapter.take() {
            reg.by_adapter.remove(&(a as usize));
        }
    }
}

unsafe extern "system" fn evt_device_prepare_hardware(
    device: WDFDEVICE,
    _raw: WDFCMRESLIST,
    _translated: WDFCMRESLIST,
) -> NTSTATUS {
    let ctx = match registry().lock().unwrap().by_device.get(&(device as usize)).cloned() {
        Some(c) => c,
        None => return STATUS_INVALID_PARAMETER,
    };

    let usb = match UsbTransport::open(device) {
        Ok(u) => u,
        Err(st) => {
            trace_log(&format!("PrepareHardware: UsbTransport::open FAILED 0x{:08X}", st as u32));
            return st;
        }
    };

    let edid = usb.read_edid_or_fallback();
    let modes = build_modes_from_edid(&edid);
    let preferred = modes[0];
    trace_log(&format!(
        "PrepareHardware: USB OK, preferred={}x{} @ {}.{:03} Hz",
        preferred.hdisplay,
        preferred.vdisplay,
        preferred.refresh_mhz() / 1000,
        preferred.refresh_mhz() % 1000
    ));

    let mut c = ctx.lock().unwrap();
    c.edid = edid;
    c.supported_modes = modes;
    c.active_mode = preferred;
    *c.usb.lock().unwrap() = Some(usb);

    STATUS_SUCCESS
}

unsafe extern "system" fn evt_device_release_hardware(
    device: WDFDEVICE,
    _translated: WDFCMRESLIST,
) -> NTSTATUS {
    let ctx = registry().lock().unwrap().by_device.get(&(device as usize)).cloned();
    if let Some(ctx) = ctx {
        let mut c = ctx.lock().unwrap();
        if let Some(proc) = c.swapchain_proc.take() {
            proc.stop();
        }
        if let Ok(mut usb_guard) = c.usb.lock() {
            if let Some(ref mut usb) = *usb_guard {
                let _ = usb.set_blank_mode(BlankMode::PowerDown);
                usb.shutdown();
            }
            *usb_guard = None;
        };
    }
    STATUS_SUCCESS
}

unsafe extern "system" fn evt_device_surprise_removal(device: WDFDEVICE) {
    let ctx = registry().lock().unwrap().by_device.get(&(device as usize)).cloned();
    if let Some(ctx) = ctx {
        let mut c = ctx.lock().unwrap();
        if let Some(proc) = c.swapchain_proc.take() {
            proc.stop();
        }
        if let Ok(mut usb_guard) = c.usb.lock() {
            if let Some(ref mut usb) = *usb_guard {
                usb.shutdown();
            }
        };
    }
}

unsafe extern "system" fn evt_device_d0_entry(
    device: WDFDEVICE,
    _previous_state: u32,
) -> NTSTATUS {
    let ctx = match registry().lock().unwrap().by_device.get(&(device as usize)).cloned() {
        Some(c) => c,
        None => return STATUS_INVALID_PARAMETER,
    };

    let need_init = {
        let mut c = ctx.lock().unwrap();
        if let Ok(mut usb_guard) = c.usb.lock() {
            if let Some(ref mut usb) = *usb_guard {
                usb.restart();
            }
        }
        if !c.adapter_initialized {
            c.adapter_initialized = true;
            true
        } else {
            false
        }
    };

    if need_init {
        let mut hw_ver = IDDCX_ENDPOINT_VERSION {
            size: size_of::<IDDCX_ENDPOINT_VERSION>() as u32,
            major_ver: 1,
            minor_ver: 0,
            build: 0,
            sku: 0x04A3,
        };
        let mut fw_ver = IDDCX_ENDPOINT_VERSION {
            size: size_of::<IDDCX_ENDPOINT_VERSION>() as u32,
            major_ver: 0,
            minor_ver: 1,
            build: 0,
            sku: 0,
        };
        let mut caps = IDDCX_ADAPTER_CAPS {
            size: size_of::<IDDCX_ADAPTER_CAPS>() as u32,
            flags: IDDCX_ADAPTER_FLAGS_USE_SMALLEST_MODE
                | IDDCX_ADAPTER_FLAGS_CAN_USE_MOVE_REGIONS
                | IDDCX_ADAPTER_FLAGS_PREFER_PRECISE_PRESENT_REGIONS,
            max_display_pipeline_rate: 1920 * 1200 * 60,
            max_monitors_supported: 1,
            _pad0: 0,
            endpoint_diagnostics: IDDCX_ENDPOINT_DIAGNOSTIC_INFO {
                size: size_of::<IDDCX_ENDPOINT_DIAGNOSTIC_INFO>() as u32,
                transmission_type: IDDCX_TRANSMISSION_TYPE_WIRED_USB,
                endpoint_friendly_name: ENDPOINT_FRIENDLY.as_ptr(),
                endpoint_model_name: ENDPOINT_MODEL.as_ptr(),
                endpoint_manufacturer_name: ENDPOINT_MFG.as_ptr(),
                hardware_version: &mut hw_ver,
                firmware_version: &mut fw_ver,
                gamma_support: IDDCX_FEATURE_IMPLEMENTATION_NONE,
                _pad0: 0,
            },
            static_desktop_reencode_frame_count: 4,
            _pad1: 0,
        };

        let in_args = IDARG_IN_ADAPTER_INIT {
            wdf_device: device,
            caps: &mut caps,
            object_attributes: core::ptr::null_mut(),
        };
        let mut out_args = IDARG_OUT_ADAPTER_INIT {
            adapter_object: core::ptr::null_mut(),
        };

        let st = idd_cx_adapter_init_async(&in_args, &mut out_args);
        trace_log(&format!("D0Entry: IddCxAdapterInitAsync=0x{:08X}", st as u32));
        if !nt_success(st) {
            return st;
        }

        ctx.lock().unwrap().adapter = Some(out_args.adapter_object);
        registry()
            .lock()
            .unwrap()
            .by_adapter
            .insert(out_args.adapter_object as usize, ctx);
    }

    STATUS_SUCCESS
}

unsafe extern "system" fn evt_device_d0_exit(device: WDFDEVICE, _target_state: u32) -> NTSTATUS {
    let ctx = registry().lock().unwrap().by_device.get(&(device as usize)).cloned();
    if let Some(ctx) = ctx {
        let mut c = ctx.lock().unwrap();
        if let Some(proc) = c.swapchain_proc.take() {
            proc.stop();
        }
        if let Ok(mut usb_guard) = c.usb.lock() {
            if let Some(ref mut usb) = *usb_guard {
                let _ = usb.set_blank_mode(BlankMode::Blanked);
                usb.shutdown();
            }
        };
    }
    STATUS_SUCCESS
}

// =============================================================================
// IddCx 1.10 Display Adapter & Monitor Callbacks
// =============================================================================

unsafe extern "system" fn evt_idd_cx_adapter_init_finished(
    adapter: IDDCX_ADAPTER,
    in_args: *const IDARG_IN_ADAPTER_INIT_FINISHED,
) -> NTSTATUS {
    if in_args.is_null() || !nt_success((*in_args).adapter_init_status) {
        return STATUS_SUCCESS;
    }

    let ctx = match registry().lock().unwrap().by_adapter.get(&(adapter as usize)).cloned() {
        Some(c) => c,
        None => return STATUS_INVALID_PARAMETER,
    };

    let mut edid_copy = ctx.lock().unwrap().edid;
    let mut monitor_info = IDDCX_MONITOR_INFO {
        size: size_of::<IDDCX_MONITOR_INFO>() as u32,
        monitor_type: DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_WIRED,
        connector_index: 0,
        _pad0: 0,
        monitor_description: IDDCX_MONITOR_DESCRIPTION {
            size: size_of::<IDDCX_MONITOR_DESCRIPTION>() as u32,
            desc_type: IDDCX_MONITOR_DESCRIPTION_TYPE_EDID,
            data_size: edid_copy.len() as u32,
            _pad0: 0,
            data: edid_copy.as_mut_ptr() as *mut c_void,
        },
        monitor_container_id: MONITOR_CONTAINER_GUID,
    };

    let create_in = IDARG_IN_MONITORCREATE {
        object_attributes: core::ptr::null_mut(),
        monitor_info: &mut monitor_info,
    };
    let mut create_out = IDARG_OUT_MONITORCREATE {
        monitor_object: core::ptr::null_mut(),
    };

    let st = idd_cx_monitor_create(adapter, &create_in, &mut create_out);
    trace_log(&format!("AdapterInitFinished: IddCxMonitorCreate=0x{:08X}", st as u32));
    if !nt_success(st) {
        return st;
    }

    let monitor = create_out.monitor_object;
    ctx.lock().unwrap().monitor = Some(monitor);
    registry()
        .lock()
        .unwrap()
        .by_monitor
        .insert(monitor as usize, ctx);

    let mut arrival_out = IDARG_OUT_MONITORARRIVAL::default();
    let st = idd_cx_monitor_arrival(monitor, &mut arrival_out);
    trace_log(&format!("AdapterInitFinished: IddCxMonitorArrival=0x{:08X}", st as u32));
    st
}

unsafe extern "system" fn evt_idd_cx_parse_monitor_description(
    in_args: *const IDARG_IN_PARSEMONITORDESCRIPTION,
    out_args: *mut IDARG_OUT_PARSEMONITORDESCRIPTION,
) -> NTSTATUS {
    if in_args.is_null() || out_args.is_null() {
        return STATUS_INVALID_PARAMETER;
    }

    let desc = &(*in_args).monitor_description;
    let modes = if !desc.data.is_null() && desc.data_size >= EDID_BLOCK_SIZE as u32 {
        let slice = core::slice::from_raw_parts(desc.data as *const u8, desc.data_size as usize);
        build_modes_from_edid(slice)
    } else {
        vec![VideoMode::DTU_1031_1280X800_60]
    };

    (*out_args).monitor_mode_buffer_output_count = modes.len() as u32;
    (*out_args).preferred_monitor_mode_idx = 0;

    if (*in_args).monitor_mode_buffer_input_count == 0 || (*in_args).monitor_modes.is_null() {
        return STATUS_SUCCESS;
    }
    if ((*in_args).monitor_mode_buffer_input_count as usize) < modes.len() {
        return STATUS_BUFFER_TOO_SMALL;
    }

    for (i, m) in modes.iter().enumerate() {
        *(*in_args).monitor_modes.add(i) = IDDCX_MONITOR_MODE {
            size: size_of::<IDDCX_MONITOR_MODE>() as u32,
            origin: IDDCX_MONITOR_MODE_ORIGIN_MONITORDESCRIPTOR,
            monitor_video_signal_info: fill_video_signal_info(m, false),
        };
    }

    STATUS_SUCCESS
}

unsafe extern "system" fn evt_idd_cx_monitor_get_default_description_modes(
    monitor: IDDCX_MONITOR,
    in_args: *const IDARG_IN_GETDEFAULTDESCRIPTIONMODES,
    out_args: *mut IDARG_OUT_GETDEFAULTDESCRIPTIONMODES,
) -> NTSTATUS {
    if in_args.is_null() || out_args.is_null() {
        return STATUS_INVALID_PARAMETER;
    }

    let modes = registry()
        .lock()
        .unwrap()
        .by_monitor
        .get(&(monitor as usize))
        .map(|c| c.lock().unwrap().supported_modes.clone())
        .unwrap_or_else(|| vec![VideoMode::DTU_1031_1280X800_60]);

    (*out_args).default_monitor_mode_buffer_output_count = modes.len() as u32;
    (*out_args).preferred_monitor_mode_idx = 0;

    if (*in_args).default_monitor_mode_buffer_input_count == 0
        || (*in_args).default_monitor_modes.is_null()
    {
        return STATUS_SUCCESS;
    }
    if ((*in_args).default_monitor_mode_buffer_input_count as usize) < modes.len() {
        return STATUS_BUFFER_TOO_SMALL;
    }

    for (i, m) in modes.iter().enumerate() {
        *(*in_args).default_monitor_modes.add(i) = IDDCX_MONITOR_MODE {
            size: size_of::<IDDCX_MONITOR_MODE>() as u32,
            origin: IDDCX_MONITOR_MODE_ORIGIN_DRIVER,
            monitor_video_signal_info: fill_video_signal_info(m, false),
        };
    }

    STATUS_SUCCESS
}

unsafe extern "system" fn evt_idd_cx_monitor_query_target_modes(
    monitor: IDDCX_MONITOR,
    in_args: *const IDARG_IN_QUERYTARGETMODES,
    out_args: *mut IDARG_OUT_QUERYTARGETMODES,
) -> NTSTATUS {
    if in_args.is_null() || out_args.is_null() {
        return STATUS_INVALID_PARAMETER;
    }

    let desc = &(*in_args).monitor_description;
    let modes = if !desc.data.is_null() && desc.data_size >= EDID_BLOCK_SIZE as u32 {
        let slice = core::slice::from_raw_parts(desc.data as *const u8, desc.data_size as usize);
        build_modes_from_edid(slice)
    } else {
        registry()
            .lock()
            .unwrap()
            .by_monitor
            .get(&(monitor as usize))
            .map(|c| c.lock().unwrap().supported_modes.clone())
            .unwrap_or_else(|| vec![VideoMode::DTU_1031_1280X800_60])
    };

    (*out_args).target_mode_buffer_output_count = modes.len() as u32;

    if (*in_args).target_mode_buffer_input_count == 0 || (*in_args).target_modes.is_null() {
        return STATUS_SUCCESS;
    }
    if ((*in_args).target_mode_buffer_input_count as usize) < modes.len() {
        return STATUS_BUFFER_TOO_SMALL;
    }

    for (i, m) in modes.iter().enumerate() {
        *(*in_args).target_modes.add(i) = IDDCX_TARGET_MODE {
            size: size_of::<IDDCX_TARGET_MODE>() as u32,
            _pad0: 0,
            target_video_signal_info: fill_video_signal_info(m, true),
            required_bandwidth: m.pixel_count() as u64 * 60,
        };
    }

    STATUS_SUCCESS
}

unsafe extern "system" fn evt_idd_cx_adapter_commit_modes(
    adapter: IDDCX_ADAPTER,
    in_args: *const IDARG_IN_COMMITMODES,
) -> NTSTATUS {
    if in_args.is_null() {
        return STATUS_INVALID_PARAMETER;
    }

    let ctx = match registry().lock().unwrap().by_adapter.get(&(adapter as usize)).cloned() {
        Some(c) => c,
        None => return STATUS_SUCCESS,
    };

    let count = (*in_args).path_count as usize;
    let mut active_mode: Option<VideoMode> = None;

    if count > 0 && !(*in_args).paths.is_null() {
        let paths = core::slice::from_raw_parts((*in_args).paths, count);
        for p in paths {
            if (p.flags & IDDCX_PATH_FLAGS_ACTIVE) != 0 {
                let w = p.target_video_signal_info.active_size.cx as u16;
                let h = p.target_video_signal_info.active_size.cy as u16;
                let c = ctx.lock().unwrap();
                let matched = c
                    .supported_modes
                    .iter()
                    .copied()
                    .find(|m| m.hdisplay == w && m.vdisplay == h)
                    .unwrap_or(c.active_mode);
                active_mode = Some(matched);
                break;
            }
        }
    }

    let mut c = ctx.lock().unwrap();
    if let Some(m) = active_mode {
        c.active_mode = m;
        if let Ok(mut usb_guard) = c.usb.lock() {
            if let Some(ref mut usb) = *usb_guard {
                let _ = usb.set_video_mode(&m);
            }
        }
    } else if let Ok(mut usb_guard) = c.usb.lock() {
        if let Some(ref mut usb) = *usb_guard {
            let _ = usb.set_blank_mode(BlankMode::Blanked);
        }
    }

    STATUS_SUCCESS
}

unsafe extern "system" fn evt_idd_cx_monitor_assign_swap_chain(
    monitor: IDDCX_MONITOR,
    in_args: *const IDARG_IN_SETSWAPCHAIN,
) -> NTSTATUS {
    if in_args.is_null() {
        return STATUS_INVALID_PARAMETER;
    }

    let ctx = match registry().lock().unwrap().by_monitor.get(&(monitor as usize)).cloned() {
        Some(c) => c,
        None => return STATUS_INVALID_PARAMETER,
    };

    let mut c = ctx.lock().unwrap();
    // Stop any previous swapchain processor cleanly before starting the new one.
    if let Some(prev) = c.swapchain_proc.take() {
        prev.stop();
    }

    let mode = c.active_mode;
    let usb = Arc::clone(&c.usb);
    match SwapChainProcessor::start(
        (*in_args).h_swap_chain,
        (*in_args).h_next_surface_available,
        (*in_args).render_adapter_luid,
        mode,
        usb,
    ) {
        Ok(proc) => {
            c.swapchain_proc = Some(proc);
            STATUS_SUCCESS
        }
        Err(st) => st,
    }
}

unsafe extern "system" fn evt_idd_cx_monitor_unassign_swap_chain(
    monitor: IDDCX_MONITOR,
) -> NTSTATUS {
    let ctx = registry().lock().unwrap().by_monitor.get(&(monitor as usize)).cloned();
    if let Some(ctx) = ctx {
        let mut c = ctx.lock().unwrap();
        if let Some(proc) = c.swapchain_proc.take() {
            proc.stop();
        }
    }
    STATUS_SUCCESS
}
