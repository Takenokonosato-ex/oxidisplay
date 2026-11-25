//! ABI-verified pure-Rust FFI bindings to Windows UMDF 2.33 (`WdfDriverStubUm.lib`)
//! and Indirect Display Class Extension 1.10 (`iddcxstub.lib`).
//!
//! Every `#[repr(C)]` struct size is compile-time asserted against the official
//! Windows WDK 10.0.28000.0 (`wdf/umdf/2.33`, `iddcx/1.10`) headers.

#![allow(non_camel_case_types, non_snake_case, dead_code, clippy::upper_case_acronyms)]

use core::ffi::c_void;
use core::mem::size_of;

pub type NTSTATUS = i32;
pub type HRESULT = i32;
pub type BOOL = i32;
pub type BOOLEAN = u8;
pub type HANDLE = *mut c_void;

pub type WDFDRIVER = *mut c_void;
pub type WDFDEVICE = *mut c_void;
pub type PWDFDEVICE_INIT = *mut c_void;
pub type WDFUSBDEVICE = *mut c_void;
pub type WDFUSBINTERFACE = *mut c_void;
pub type WDFUSBPIPE = *mut c_void;
pub type WDFIOTARGET = *mut c_void;
pub type WDFREQUEST = *mut c_void;
pub type WDFMEMORY = *mut c_void;
pub type WDFOBJECT = *mut c_void;
pub type WDFCMRESLIST = *mut c_void;

pub type IDDCX_ADAPTER = *mut c_void;
pub type IDDCX_MONITOR = *mut c_void;
pub type IDDCX_SWAPCHAIN = *mut c_void;

pub const STATUS_SUCCESS: NTSTATUS = 0;
pub const STATUS_UNSUCCESSFUL: NTSTATUS = 0xC000_0001u32 as i32;
pub const STATUS_INVALID_PARAMETER: NTSTATUS = 0xC000_000Du32 as i32;
pub const STATUS_NO_MEMORY: NTSTATUS = 0xC000_0017u32 as i32;
pub const STATUS_BUFFER_TOO_SMALL: NTSTATUS = 0xC000_0023u32 as i32;
pub const STATUS_DEVICE_NOT_CONNECTED: NTSTATUS = 0xC000_009Du32 as i32;

pub const S_OK: HRESULT = 0;
pub const E_PENDING: HRESULT = 0x8000_000Au32 as i32;

pub const NO_PREFERRED_MODE: u32 = 0xFFFF_FFFF;

pub const USBD_CLIENT_CONTRACT_VERSION_602: u32 = 0x602;
pub const WDF_USB_SELECT_CONFIG_SINGLE_INTERFACE: u32 = 2;
pub const WDF_MEMORY_DESCRIPTOR_TYPE_BUFFER: u32 = 1;
pub const WDF_USB_PIPE_TYPE_BULK: u32 = 3;
pub const WDF_REQUEST_SEND_OPTION_TIMEOUT: u32 = 0x1;
pub const WDF_REQUEST_SEND_OPTION_SYNCHRONOUS: u32 = 0x2;
pub const WDF_IO_TARGET_CANCEL_SENT_IO: u32 = 1;

pub const DISPLAYCONFIG_OUTPUT_TECHNOLOGY_OTHER: i32 = -1;
pub const DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INDIRECT_WIRED: i32 = 16;
pub const DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL: i32 = i32::MIN;
pub const DISPLAYCONFIG_SCANLINE_ORDERING_PROGRESSIVE: u32 = 1;
pub const D3DKMDT_VSS_OTHER: u32 = 255;

pub const IDDCX_TRANSMISSION_TYPE_WIRED_USB: u32 = 1;
pub const IDDCX_FEATURE_IMPLEMENTATION_NONE: u32 = 1;
pub const IDDCX_MONITOR_DESCRIPTION_TYPE_EDID: u32 = 1;
pub const IDDCX_MONITOR_MODE_ORIGIN_MONITORDESCRIPTOR: u32 = 1;
pub const IDDCX_MONITOR_MODE_ORIGIN_DRIVER: u32 = 2;

pub const IDDCX_ADAPTER_FLAGS_USE_SMALLEST_MODE: u32 = 0x1;
pub const IDDCX_ADAPTER_FLAGS_CAN_USE_MOVE_REGIONS: u32 = 0x2;
pub const IDDCX_ADAPTER_FLAGS_PREFER_PRECISE_PRESENT_REGIONS: u32 = 0x20;

pub const IDDCX_PATH_FLAGS_CHANGED: u32 = 0x1;
pub const IDDCX_PATH_FLAGS_ACTIVE: u32 = 0x2;

#[inline]
pub const fn nt_success(status: NTSTATUS) -> bool {
    status >= 0
}

#[inline]
pub const fn succeeded(hr: HRESULT) -> bool {
    hr >= 0
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct LUID {
    pub low_part: u32,
    pub high_part: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UNICODE_STRING {
    pub length: u16,
    pub maximum_length: u16,
    pub buffer: *mut u16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct WDF_DRIVER_CONFIG {
    pub size: u32,
    pub _pad0: u32,
    pub evt_driver_device_add: Option<unsafe extern "system" fn(WDFDRIVER, PWDFDEVICE_INIT) -> NTSTATUS>,
    pub evt_driver_unload: Option<unsafe extern "system" fn(WDFDRIVER)>,
    pub driver_init_flags: u32,
    pub driver_pool_tag: u32,
}
const _: () = assert!(size_of::<WDF_DRIVER_CONFIG>() == 32);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WDF_OBJECT_ATTRIBUTES {
    pub size: u32,
    pub _pad0: u32,
    pub evt_cleanup_callback: Option<unsafe extern "system" fn(WDFOBJECT)>,
    pub evt_destroy_callback: Option<unsafe extern "system" fn(WDFOBJECT)>,
    pub execution_level: u32,
    pub synchronization_scope: u32,
    pub parent_object: WDFOBJECT,
    pub context_size_override: usize,
    pub context_type_info: *const c_void,
}
const _: () = assert!(size_of::<WDF_OBJECT_ATTRIBUTES>() == 56);

impl WDF_OBJECT_ATTRIBUTES {
    pub fn init(parent: WDFOBJECT) -> Self {
        Self {
            size: size_of::<Self>() as u32,
            _pad0: 0,
            evt_cleanup_callback: None,
            evt_destroy_callback: None,
            execution_level: 1,       // WdfExecutionLevelInheritFromParent
            synchronization_scope: 1, // WdfSynchronizationScopeInheritFromParent
            parent_object: parent,
            context_size_override: 0,
            context_type_info: core::ptr::null(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WDF_PNPPOWER_EVENT_CALLBACKS {
    pub size: u32,
    pub _pad0: u32,
    pub evt_device_d0_entry: Option<unsafe extern "system" fn(WDFDEVICE, u32) -> NTSTATUS>,
    pub evt_device_d0_entry_post_interrupts_enabled: *const c_void,
    pub evt_device_d0_exit: Option<unsafe extern "system" fn(WDFDEVICE, u32) -> NTSTATUS>,
    pub evt_device_d0_exit_pre_interrupts_disabled: *const c_void,
    pub evt_device_prepare_hardware: Option<unsafe extern "system" fn(WDFDEVICE, WDFCMRESLIST, WDFCMRESLIST) -> NTSTATUS>,
    pub evt_device_release_hardware: Option<unsafe extern "system" fn(WDFDEVICE, WDFCMRESLIST) -> NTSTATUS>,
    pub evt_device_self_managed_io_cleanup: *const c_void,
    pub evt_device_self_managed_io_flush: *const c_void,
    pub evt_device_self_managed_io_init: Option<unsafe extern "system" fn(WDFDEVICE) -> NTSTATUS>,
    pub evt_device_self_managed_io_suspend: *const c_void,
    pub evt_device_self_managed_io_restart: *const c_void,
    pub evt_device_surprise_removal: Option<unsafe extern "system" fn(WDFDEVICE)>,
    pub evt_device_query_remove: *const c_void,
    pub evt_device_query_stop: *const c_void,
    pub evt_device_usage_notification: *const c_void,
    pub evt_device_relations_query: *const c_void,
    pub evt_device_usage_notification_ex: *const c_void,
}
const _: () = assert!(size_of::<WDF_PNPPOWER_EVENT_CALLBACKS>() == 144);

impl Default for WDF_PNPPOWER_EVENT_CALLBACKS {
    fn default() -> Self {
        let mut s: Self = unsafe { core::mem::zeroed() };
        s.size = size_of::<Self>() as u32;
        s
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WDF_USB_DEVICE_CREATE_CONFIG {
    pub size: u32,
    pub usbd_client_contract_version: u32,
}
const _: () = assert!(size_of::<WDF_USB_DEVICE_CREATE_CONFIG>() == 8);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WDF_USB_DEVICE_SELECT_CONFIG_PARAMS {
    pub size: u32,
    pub config_type: u32,
    pub number_configured_pipes: u8,
    pub _pad: [u8; 7],
    pub configured_usb_interface: WDFUSBINTERFACE,
    pub _reserved: usize,
}
const _: () = assert!(size_of::<WDF_USB_DEVICE_SELECT_CONFIG_PARAMS>() == 32);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WDF_USB_CONTROL_SETUP_PACKET {
    pub bm_request_type: u8,
    pub b_request: u8,
    pub w_value: u16,
    pub w_index: u16,
    pub w_length: u16,
}
const _: () = assert!(size_of::<WDF_USB_CONTROL_SETUP_PACKET>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct WDF_USB_PIPE_INFORMATION {
    pub size: u32,
    pub maximum_packet_size: u32,
    pub endpoint_address: u8,
    pub interval: u8,
    pub _pad: [u8; 2],
    pub pipe_type: u32,
    pub maximum_transfer_size: u32,
}
const _: () = assert!(size_of::<WDF_USB_PIPE_INFORMATION>() == 20);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WDF_MEMORY_DESCRIPTOR {
    pub desc_type: u32,
    pub _pad0: u32,
    pub buffer: *mut c_void,
    pub length: u32,
    pub _pad1: u32,
}
const _: () = assert!(size_of::<WDF_MEMORY_DESCRIPTOR>() == 24);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WDF_REQUEST_SEND_OPTIONS {
    pub size: u32,
    pub flags: u32,
    pub timeout: i64,
}
const _: () = assert!(size_of::<WDF_REQUEST_SEND_OPTIONS>() == 16);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WDF_REQUEST_REUSE_PARAMS {
    pub size: u32,
    pub flags: u32,
    pub status: NTSTATUS,
    pub _pad0: u32,
    pub new_irp: *mut c_void,
}
const _: () = assert!(size_of::<WDF_REQUEST_REUSE_PARAMS>() == 24);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WDFMEMORY_OFFSET {
    pub buffer_offset: usize,
    pub buffer_length: usize,
}
const _: () = assert!(size_of::<WDFMEMORY_OFFSET>() == 16);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WDF_REQUEST_COMPLETION_PARAMS {
    pub size: u32,
    pub req_type: u32,
    pub io_status_status: NTSTATUS,
    pub _pad0: u32,
    pub io_status_info: usize,
    pub _params: [usize; 6],
}
const _: () = assert!(size_of::<WDF_REQUEST_COMPLETION_PARAMS>() == 72);

// =============================================================================
// IddCx 1.10 Structures (ABI verified against WDK 10.0.28000.0 IddCx.h)
// =============================================================================

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DISPLAYCONFIG_RATIONAL {
    pub numerator: u32,
    pub denominator: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DISPLAYCONFIG_2DREGION {
    pub cx: u32,
    pub cy: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct DISPLAYCONFIG_VIDEO_SIGNAL_INFO {
    pub pixel_rate: u64,
    pub h_sync_freq: DISPLAYCONFIG_RATIONAL,
    pub v_sync_freq: DISPLAYCONFIG_RATIONAL,
    pub active_size: DISPLAYCONFIG_2DREGION,
    pub total_size: DISPLAYCONFIG_2DREGION,
    /// Bitfield union: bits 0..16 = videoStandard, bits 16..22 = vSyncFreqDivider (MUST be >= 1 for target modes; 0 for monitor modes!).
    pub additional_signal_info: u32,
    pub scan_line_ordering: u32,
}
const _: () = assert!(size_of::<DISPLAYCONFIG_VIDEO_SIGNAL_INFO>() == 48);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDD_CX_CLIENT_CONFIG {
    pub size: u32,
    pub _pad0: u32,
    pub evt_idd_cx_device_io_control: *const c_void,
    pub evt_idd_cx_parse_monitor_description: Option<
        unsafe extern "system" fn(
            *const IDARG_IN_PARSEMONITORDESCRIPTION,
            *mut IDARG_OUT_PARSEMONITORDESCRIPTION,
        ) -> NTSTATUS,
    >,
    pub evt_idd_cx_adapter_init_finished: Option<
        unsafe extern "system" fn(IDDCX_ADAPTER, *const IDARG_IN_ADAPTER_INIT_FINISHED) -> NTSTATUS,
    >,
    pub evt_idd_cx_adapter_commit_modes: Option<
        unsafe extern "system" fn(IDDCX_ADAPTER, *const IDARG_IN_COMMITMODES) -> NTSTATUS,
    >,
    pub evt_idd_cx_monitor_get_default_description_modes: Option<
        unsafe extern "system" fn(
            IDDCX_MONITOR,
            *const IDARG_IN_GETDEFAULTDESCRIPTIONMODES,
            *mut IDARG_OUT_GETDEFAULTDESCRIPTIONMODES,
        ) -> NTSTATUS,
    >,
    pub evt_idd_cx_monitor_query_target_modes: Option<
        unsafe extern "system" fn(
            IDDCX_MONITOR,
            *const IDARG_IN_QUERYTARGETMODES,
            *mut IDARG_OUT_QUERYTARGETMODES,
        ) -> NTSTATUS,
    >,
    pub evt_idd_cx_monitor_assign_swap_chain: Option<
        unsafe extern "system" fn(IDDCX_MONITOR, *const IDARG_IN_SETSWAPCHAIN) -> NTSTATUS,
    >,
    pub evt_idd_cx_monitor_unassign_swap_chain: Option<
        unsafe extern "system" fn(IDDCX_MONITOR) -> NTSTATUS,
    >,
    pub _optional_callbacks: [*const c_void; 17],
}
const _: () = assert!(size_of::<IDD_CX_CLIENT_CONFIG>() == 208);

impl Default for IDD_CX_CLIENT_CONFIG {
    fn default() -> Self {
        let mut s: Self = unsafe { core::mem::zeroed() };
        s.size = size_of::<Self>() as u32;
        s
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDDCX_ENDPOINT_VERSION {
    pub size: u32,
    pub major_ver: u32,
    pub minor_ver: u32,
    pub build: u32,
    pub sku: u64,
}
const _: () = assert!(size_of::<IDDCX_ENDPOINT_VERSION>() == 24);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDDCX_ENDPOINT_DIAGNOSTIC_INFO {
    pub size: u32,
    pub transmission_type: u32,
    pub endpoint_friendly_name: *const u16,
    pub endpoint_model_name: *const u16,
    pub endpoint_manufacturer_name: *const u16,
    pub hardware_version: *mut IDDCX_ENDPOINT_VERSION,
    pub firmware_version: *mut IDDCX_ENDPOINT_VERSION,
    pub gamma_support: u32,
    pub _pad0: u32,
}
const _: () = assert!(size_of::<IDDCX_ENDPOINT_DIAGNOSTIC_INFO>() == 56);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDDCX_ADAPTER_CAPS {
    pub size: u32,
    pub flags: u32,
    pub max_display_pipeline_rate: u64,
    pub max_monitors_supported: u32,
    pub _pad0: u32,
    pub endpoint_diagnostics: IDDCX_ENDPOINT_DIAGNOSTIC_INFO,
    pub static_desktop_reencode_frame_count: u32,
    pub _pad1: u32,
}
const _: () = assert!(size_of::<IDDCX_ADAPTER_CAPS>() == 88);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_IN_ADAPTER_INIT {
    pub wdf_device: WDFDEVICE,
    pub caps: *mut IDDCX_ADAPTER_CAPS,
    pub object_attributes: *mut WDF_OBJECT_ATTRIBUTES,
}
const _: () = assert!(size_of::<IDARG_IN_ADAPTER_INIT>() == 24);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_OUT_ADAPTER_INIT {
    pub adapter_object: IDDCX_ADAPTER,
}
const _: () = assert!(size_of::<IDARG_OUT_ADAPTER_INIT>() == 8);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_IN_ADAPTER_INIT_FINISHED {
    pub adapter_init_status: NTSTATUS,
}
const _: () = assert!(size_of::<IDARG_IN_ADAPTER_INIT_FINISHED>() == 4);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDDCX_MONITOR_DESCRIPTION {
    pub size: u32,
    pub desc_type: u32,
    pub data_size: u32,
    pub _pad0: u32,
    pub data: *mut c_void,
}
const _: () = assert!(size_of::<IDDCX_MONITOR_DESCRIPTION>() == 24);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDDCX_MONITOR_INFO {
    pub size: u32,
    pub monitor_type: i32,
    pub connector_index: u32,
    pub _pad0: u32,
    pub monitor_description: IDDCX_MONITOR_DESCRIPTION,
    pub monitor_container_id: GUID,
}
const _: () = assert!(size_of::<IDDCX_MONITOR_INFO>() == 56);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_IN_MONITORCREATE {
    pub object_attributes: *mut WDF_OBJECT_ATTRIBUTES,
    pub monitor_info: *mut IDDCX_MONITOR_INFO,
}
const _: () = assert!(size_of::<IDARG_IN_MONITORCREATE>() == 16);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_OUT_MONITORCREATE {
    pub monitor_object: IDDCX_MONITOR,
}
const _: () = assert!(size_of::<IDARG_OUT_MONITORCREATE>() == 8);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IDARG_OUT_MONITORARRIVAL {
    pub os_adapter_luid: LUID,
    pub os_target_id: u32,
}
const _: () = assert!(size_of::<IDARG_OUT_MONITORARRIVAL>() == 12);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IDDCX_MONITOR_MODE {
    pub size: u32,
    pub origin: u32,
    pub monitor_video_signal_info: DISPLAYCONFIG_VIDEO_SIGNAL_INFO,
}
const _: () = assert!(size_of::<IDDCX_MONITOR_MODE>() == 56);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IDDCX_TARGET_MODE {
    pub size: u32,
    pub _pad0: u32,
    pub target_video_signal_info: DISPLAYCONFIG_VIDEO_SIGNAL_INFO,
    pub required_bandwidth: u64,
}
const _: () = assert!(size_of::<IDDCX_TARGET_MODE>() == 64);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_IN_PARSEMONITORDESCRIPTION {
    pub monitor_description: IDDCX_MONITOR_DESCRIPTION,
    pub monitor_mode_buffer_input_count: u32,
    pub _pad0: u32,
    pub monitor_modes: *mut IDDCX_MONITOR_MODE,
}
const _: () = assert!(size_of::<IDARG_IN_PARSEMONITORDESCRIPTION>() == 40);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_OUT_PARSEMONITORDESCRIPTION {
    pub monitor_mode_buffer_output_count: u32,
    pub preferred_monitor_mode_idx: u32,
}
const _: () = assert!(size_of::<IDARG_OUT_PARSEMONITORDESCRIPTION>() == 8);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_IN_GETDEFAULTDESCRIPTIONMODES {
    pub default_monitor_mode_buffer_input_count: u32,
    pub _pad0: u32,
    pub default_monitor_modes: *mut IDDCX_MONITOR_MODE,
}
const _: () = assert!(size_of::<IDARG_IN_GETDEFAULTDESCRIPTIONMODES>() == 16);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_OUT_GETDEFAULTDESCRIPTIONMODES {
    pub default_monitor_mode_buffer_output_count: u32,
    pub preferred_monitor_mode_idx: u32,
}
const _: () = assert!(size_of::<IDARG_OUT_GETDEFAULTDESCRIPTIONMODES>() == 8);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_IN_QUERYTARGETMODES {
    pub monitor_description: IDDCX_MONITOR_DESCRIPTION,
    pub target_mode_buffer_input_count: u32,
    pub _pad0: u32,
    pub target_modes: *mut IDDCX_TARGET_MODE,
}
const _: () = assert!(size_of::<IDARG_IN_QUERYTARGETMODES>() == 40);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_OUT_QUERYTARGETMODES {
    pub target_mode_buffer_output_count: u32,
}
const _: () = assert!(size_of::<IDARG_OUT_QUERYTARGETMODES>() == 4);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDDCX_PATH {
    pub size: u32,
    pub _pad0: u32,
    pub monitor_object: IDDCX_MONITOR,
    pub flags: u32,
    pub _pad1: u32,
    pub target_video_signal_info: DISPLAYCONFIG_VIDEO_SIGNAL_INFO,
}
const _: () = assert!(size_of::<IDDCX_PATH>() == 72);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_IN_COMMITMODES {
    pub path_count: u32,
    pub _pad0: u32,
    pub paths: *mut IDDCX_PATH,
}
const _: () = assert!(size_of::<IDARG_IN_COMMITMODES>() == 16);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_IN_SETSWAPCHAIN {
    pub h_swap_chain: IDDCX_SWAPCHAIN,
    pub h_next_surface_available: HANDLE,
    pub render_adapter_luid: LUID,
}
const _: () = assert!(size_of::<IDARG_IN_SETSWAPCHAIN>() == 24);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_IN_SWAPCHAINSETDEVICE {
    pub device: *mut c_void, // IDXGIDevice*
}
const _: () = assert!(size_of::<IDARG_IN_SWAPCHAINSETDEVICE>() == 8);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDDCX_METADATA {
    pub size: u32,
    pub presentation_frame_number: u32,
    pub dirty_rect_count: u32,
    pub move_region_count: u32,
    pub hw_protected_surface: BOOL,
    pub _pad0: u32,
    pub present_display_qpc_time: u64,
    pub surface: *mut c_void, // IDXGIResource*
}
const _: () = assert!(size_of::<IDDCX_METADATA>() == 40);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_OUT_RELEASEANDACQUIREBUFFER {
    pub meta_data: IDDCX_METADATA,
}
const _: () = assert!(size_of::<IDARG_OUT_RELEASEANDACQUIREBUFFER>() == 40);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDDCX_SYSTEM_BUFFER_INFO {
    pub size: u32,
    pub pitch: u32,
    pub height: u32,
    pub width: u32,
    pub format: u32,
    pub _pad0: u32,
    pub buffer: *mut c_void,
}
const _: () = assert!(size_of::<IDDCX_SYSTEM_BUFFER_INFO>() == 32);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_OUT_RELEASEANDACQUIRESYSTEMBUFFER {
    pub meta_data: *mut IDDCX_METADATA,
    pub buffer_info: *mut IDDCX_SYSTEM_BUFFER_INFO,
}
const _: () = assert!(size_of::<IDARG_OUT_RELEASEANDACQUIRESYSTEMBUFFER>() == 16);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_IN_GETDIRTYRECTS {
    pub dirty_rect_in_count: u32,
    pub _pad0: u32,
    pub dirty_rects: *mut RECT,
}
const _: () = assert!(size_of::<IDARG_IN_GETDIRTYRECTS>() == 16);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_OUT_GETDIRTYRECTS {
    pub dirty_rect_out_count: u32,
}
const _: () = assert!(size_of::<IDARG_OUT_GETDIRTYRECTS>() == 4);

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IDDCX_MOVEREGION {
    pub size: u32,
    pub source_point: POINT,
    pub dest_rect: RECT,
}
const _: () = assert!(size_of::<IDDCX_MOVEREGION>() == 28);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_IN_GETMOVEREGIONS {
    pub move_region_in_count: u32,
    pub _pad0: u32,
    pub move_regions: *mut IDDCX_MOVEREGION,
}
const _: () = assert!(size_of::<IDARG_IN_GETMOVEREGIONS>() == 16);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_OUT_GETMOVEREGIONS {
    pub move_region_out_count: u32,
}
const _: () = assert!(size_of::<IDARG_OUT_GETMOVEREGIONS>() == 4);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct IDARG_IN_SETREALTIMEGPUPRIORITY {
    pub device: *mut c_void, // IDXGIDevice*
}
const _: () = assert!(size_of::<IDARG_IN_SETREALTIMEGPUPRIORITY>() == 8);

// =============================================================================
// WDF 2.33 & IddCx 1.10 Dispatch Table Globals & Safe-ish Rust Wrappers
// =============================================================================

/// Minimum IddCx minor version required by this driver (IddCx 1.10 -> Windows 11 22H2+/24H2+).
/// Referenced by `iddcxstub.lib`.
#[no_mangle]
pub static IddMinimumVersionRequired: u32 = 10;

extern "C" {
    pub static WdfDriverGlobals: *mut c_void;
    pub static WdfFunctions_02033: *const *const c_void;

    pub static IddDriverGlobals: *mut c_void;
    /// Declared as `extern PFN_IDD_CX IddFunctions[];` in IddCx.h (inline array of 36 function pointers).
    pub static IddFunctions: [*const c_void; 36];
}

// WDF 2.33 function table indices (verified from wdffuncenum.h)
const WDF_DEVICE_INIT_SET_PNPPOWER_CALLBACKS: usize = 19;
const WDF_DEVICE_CREATE: usize = 25;
const WDF_DRIVER_CREATE: usize = 57;
const WDF_IO_TARGET_START: usize = 106;
const WDF_IO_TARGET_STOP: usize = 107;
const WDF_MEMORY_CREATE_PREALLOCATED: usize = 118;
const WDF_OBJECT_DELETE: usize = 129;
const WDF_REQUEST_CREATE: usize = 148;
const WDF_REQUEST_REUSE: usize = 149;
const WDF_REQUEST_SEND: usize = 152;
const WDF_REQUEST_SET_COMPLETION_ROUTINE: usize = 160;
const WDF_USB_TARGET_DEVICE_CREATE_WITH_PARAMS: usize = 203;
const WDF_USB_TARGET_DEVICE_SELECT_CONFIG: usize = 211;
const WDF_USB_TARGET_DEVICE_SEND_CONTROL_SYNC: usize = 212;
const WDF_USB_TARGET_PIPE_GET_INFO: usize = 216;
const WDF_USB_TARGET_PIPE_SET_NO_MAX_PKT_CHECK: usize = 220;
const WDF_USB_TARGET_PIPE_WRITE_SYNC: usize = 221;
const WDF_USB_TARGET_PIPE_FORMAT_WRITE: usize = 222;
const WDF_USB_TARGET_PIPE_RESET_SYNC: usize = 228;
const WDF_USB_INTERFACE_GET_CONFIGURED_PIPE: usize = 239;

// IddCx 1.10 function table indices (verified from IddCxFuncEnum.h)
const IDD_DEVICE_INIT_CONFIG: usize = 0;
const IDD_DEVICE_INITIALIZE: usize = 1;
const IDD_ADAPTER_INIT_ASYNC: usize = 2;
const IDD_MONITOR_CREATE: usize = 3;
const IDD_MONITOR_ARRIVAL: usize = 4;
const IDD_MONITOR_DEPARTURE: usize = 5;
const IDD_SWAPCHAIN_SET_DEVICE: usize = 10;
const IDD_SWAPCHAIN_RELEASE_AND_ACQUIRE_BUFFER: usize = 11;
const IDD_SWAPCHAIN_GET_DIRTY_RECTS: usize = 12;
const IDD_SWAPCHAIN_GET_MOVE_REGIONS: usize = 13;
const IDD_SWAPCHAIN_FINISHED_PROCESSING_FRAME: usize = 14;
const IDD_SWAPCHAIN_IN_SYSTEM_MEMORY: usize = 26;
const IDD_SWAPCHAIN_RELEASE_AND_ACQUIRE_SYSTEM_BUFFER: usize = 27;
const IDD_SET_REALTIME_GPU_PRIORITY: usize = 30;

#[inline]
unsafe fn wdf_fn<F: Copy>(index: usize) -> F {
    let ptr = *WdfFunctions_02033.add(index);
    core::mem::transmute_copy(&ptr)
}

#[inline]
unsafe fn idd_fn<F: Copy>(index: usize) -> F {
    let ptr = IddFunctions[index];
    core::mem::transmute_copy(&ptr)
}

pub unsafe fn wdf_driver_create(
    driver_object: *mut c_void,
    registry_path: *const UNICODE_STRING,
    attrs: *mut WDF_OBJECT_ATTRIBUTES,
    config: *mut WDF_DRIVER_CONFIG,
    driver_out: *mut WDFDRIVER,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const UNICODE_STRING,
        *mut WDF_OBJECT_ATTRIBUTES,
        *mut WDF_DRIVER_CONFIG,
        *mut WDFDRIVER,
    ) -> NTSTATUS = wdf_fn(WDF_DRIVER_CREATE);
    f(WdfDriverGlobals, driver_object, registry_path, attrs, config, driver_out)
}

pub unsafe fn wdf_device_init_set_pnppower_callbacks(
    device_init: PWDFDEVICE_INIT,
    callbacks: *mut WDF_PNPPOWER_EVENT_CALLBACKS,
) {
    let f: unsafe extern "system" fn(*mut c_void, PWDFDEVICE_INIT, *mut WDF_PNPPOWER_EVENT_CALLBACKS) =
        wdf_fn(WDF_DEVICE_INIT_SET_PNPPOWER_CALLBACKS);
    f(WdfDriverGlobals, device_init, callbacks);
}

pub unsafe fn wdf_device_create(
    device_init: *mut PWDFDEVICE_INIT,
    attrs: *mut WDF_OBJECT_ATTRIBUTES,
    device_out: *mut WDFDEVICE,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        *mut PWDFDEVICE_INIT,
        *mut WDF_OBJECT_ATTRIBUTES,
        *mut WDFDEVICE,
    ) -> NTSTATUS = wdf_fn(WDF_DEVICE_CREATE);
    f(WdfDriverGlobals, device_init, attrs, device_out)
}

pub unsafe fn wdf_usb_target_device_create_with_parameters(
    device: WDFDEVICE,
    config: *mut WDF_USB_DEVICE_CREATE_CONFIG,
    attrs: *mut WDF_OBJECT_ATTRIBUTES,
    usb_dev_out: *mut WDFUSBDEVICE,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        WDFDEVICE,
        *mut WDF_USB_DEVICE_CREATE_CONFIG,
        *mut WDF_OBJECT_ATTRIBUTES,
        *mut WDFUSBDEVICE,
    ) -> NTSTATUS = wdf_fn(WDF_USB_TARGET_DEVICE_CREATE_WITH_PARAMS);
    f(WdfDriverGlobals, device, config, attrs, usb_dev_out)
}

pub unsafe fn wdf_usb_target_device_select_config(
    usb_device: WDFUSBDEVICE,
    pipe_attrs: *mut WDF_OBJECT_ATTRIBUTES,
    params: *mut WDF_USB_DEVICE_SELECT_CONFIG_PARAMS,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        WDFUSBDEVICE,
        *mut WDF_OBJECT_ATTRIBUTES,
        *mut WDF_USB_DEVICE_SELECT_CONFIG_PARAMS,
    ) -> NTSTATUS = wdf_fn(WDF_USB_TARGET_DEVICE_SELECT_CONFIG);
    f(WdfDriverGlobals, usb_device, pipe_attrs, params)
}

pub unsafe fn wdf_usb_interface_get_configured_pipe(
    usb_interface: WDFUSBINTERFACE,
    pipe_index: u8,
    pipe_info: *mut WDF_USB_PIPE_INFORMATION,
) -> WDFUSBPIPE {
    let f: unsafe extern "system" fn(
        *mut c_void,
        WDFUSBINTERFACE,
        u8,
        *mut WDF_USB_PIPE_INFORMATION,
    ) -> WDFUSBPIPE = wdf_fn(WDF_USB_INTERFACE_GET_CONFIGURED_PIPE);
    f(WdfDriverGlobals, usb_interface, pipe_index, pipe_info)
}

pub unsafe fn wdf_usb_target_pipe_set_no_max_packet_check(pipe: WDFUSBPIPE) {
    let f: unsafe extern "system" fn(*mut c_void, WDFUSBPIPE) =
        wdf_fn(WDF_USB_TARGET_PIPE_SET_NO_MAX_PKT_CHECK);
    f(WdfDriverGlobals, pipe);
}

pub unsafe fn wdf_usb_target_device_send_control_sync(
    usb_device: WDFUSBDEVICE,
    request: WDFREQUEST,
    options: *mut WDF_REQUEST_SEND_OPTIONS,
    setup_packet: *mut WDF_USB_CONTROL_SETUP_PACKET,
    memory_desc: *mut WDF_MEMORY_DESCRIPTOR,
    bytes_transferred: *mut u32,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        WDFUSBDEVICE,
        WDFREQUEST,
        *mut WDF_REQUEST_SEND_OPTIONS,
        *mut WDF_USB_CONTROL_SETUP_PACKET,
        *mut WDF_MEMORY_DESCRIPTOR,
        *mut u32,
    ) -> NTSTATUS = wdf_fn(WDF_USB_TARGET_DEVICE_SEND_CONTROL_SYNC);
    f(
        WdfDriverGlobals,
        usb_device,
        request,
        options,
        setup_packet,
        memory_desc,
        bytes_transferred,
    )
}

pub unsafe fn wdf_usb_target_pipe_write_sync(
    pipe: WDFUSBPIPE,
    request: WDFREQUEST,
    options: *mut WDF_REQUEST_SEND_OPTIONS,
    memory_desc: *mut WDF_MEMORY_DESCRIPTOR,
    bytes_written: *mut u32,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        WDFUSBPIPE,
        WDFREQUEST,
        *mut WDF_REQUEST_SEND_OPTIONS,
        *mut WDF_MEMORY_DESCRIPTOR,
        *mut u32,
    ) -> NTSTATUS = wdf_fn(WDF_USB_TARGET_PIPE_WRITE_SYNC);
    f(WdfDriverGlobals, pipe, request, options, memory_desc, bytes_written)
}

pub unsafe fn wdf_usb_target_pipe_format_request_for_write(
    pipe: WDFUSBPIPE,
    request: WDFREQUEST,
    write_memory: WDFMEMORY,
    write_offset: *mut WDFMEMORY_OFFSET,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        WDFUSBPIPE,
        WDFREQUEST,
        WDFMEMORY,
        *mut WDFMEMORY_OFFSET,
    ) -> NTSTATUS = wdf_fn(WDF_USB_TARGET_PIPE_FORMAT_WRITE);
    f(WdfDriverGlobals, pipe, request, write_memory, write_offset)
}

pub unsafe fn wdf_usb_target_pipe_reset_sync(
    pipe: WDFUSBPIPE,
    request: WDFREQUEST,
    options: *mut WDF_REQUEST_SEND_OPTIONS,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        WDFUSBPIPE,
        WDFREQUEST,
        *mut WDF_REQUEST_SEND_OPTIONS,
    ) -> NTSTATUS = wdf_fn(WDF_USB_TARGET_PIPE_RESET_SYNC);
    f(WdfDriverGlobals, pipe, request, options)
}

pub unsafe fn wdf_memory_create_preallocated(
    attrs: *mut WDF_OBJECT_ATTRIBUTES,
    buffer: *mut c_void,
    buffer_size: usize,
    memory_out: *mut WDFMEMORY,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        *mut WDF_OBJECT_ATTRIBUTES,
        *mut c_void,
        usize,
        *mut WDFMEMORY,
    ) -> NTSTATUS = wdf_fn(WDF_MEMORY_CREATE_PREALLOCATED);
    f(WdfDriverGlobals, attrs, buffer, buffer_size, memory_out)
}

pub unsafe fn wdf_request_create(
    attrs: *mut WDF_OBJECT_ATTRIBUTES,
    io_target: WDFIOTARGET,
    request_out: *mut WDFREQUEST,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        *mut WDF_OBJECT_ATTRIBUTES,
        WDFIOTARGET,
        *mut WDFREQUEST,
    ) -> NTSTATUS = wdf_fn(WDF_REQUEST_CREATE);
    f(WdfDriverGlobals, attrs, io_target, request_out)
}

pub unsafe fn wdf_request_reuse(request: WDFREQUEST, params: *mut WDF_REQUEST_REUSE_PARAMS) -> NTSTATUS {
    let f: unsafe extern "system" fn(*mut c_void, WDFREQUEST, *mut WDF_REQUEST_REUSE_PARAMS) -> NTSTATUS =
        wdf_fn(WDF_REQUEST_REUSE);
    f(WdfDriverGlobals, request, params)
}

pub unsafe fn wdf_request_set_completion_routine(
    request: WDFREQUEST,
    routine: Option<
        unsafe extern "system" fn(
            WDFREQUEST,
            WDFIOTARGET,
            *mut WDF_REQUEST_COMPLETION_PARAMS,
            *mut c_void,
        ),
    >,
    context: *mut c_void,
) {
    let f: unsafe extern "system" fn(
        *mut c_void,
        WDFREQUEST,
        Option<
            unsafe extern "system" fn(
                WDFREQUEST,
                WDFIOTARGET,
                *mut WDF_REQUEST_COMPLETION_PARAMS,
                *mut c_void,
            ),
        >,
        *mut c_void,
    ) = wdf_fn(WDF_REQUEST_SET_COMPLETION_ROUTINE);
    f(WdfDriverGlobals, request, routine, context);
}

pub unsafe fn wdf_request_send(
    request: WDFREQUEST,
    target: WDFIOTARGET,
    options: *mut WDF_REQUEST_SEND_OPTIONS,
) -> BOOLEAN {
    let f: unsafe extern "system" fn(
        *mut c_void,
        WDFREQUEST,
        WDFIOTARGET,
        *mut WDF_REQUEST_SEND_OPTIONS,
    ) -> BOOLEAN = wdf_fn(WDF_REQUEST_SEND);
    f(WdfDriverGlobals, request, target, options)
}

pub unsafe fn wdf_io_target_start(target: WDFIOTARGET) -> NTSTATUS {
    let f: unsafe extern "system" fn(*mut c_void, WDFIOTARGET) -> NTSTATUS =
        wdf_fn(WDF_IO_TARGET_START);
    f(WdfDriverGlobals, target)
}

pub unsafe fn wdf_io_target_stop(target: WDFIOTARGET, action: u32) {
    let f: unsafe extern "system" fn(*mut c_void, WDFIOTARGET, u32) = wdf_fn(WDF_IO_TARGET_STOP);
    f(WdfDriverGlobals, target, action);
}

pub unsafe fn wdf_object_delete(obj: WDFOBJECT) {
    let f: unsafe extern "system" fn(*mut c_void, WDFOBJECT) = wdf_fn(WDF_OBJECT_DELETE);
    f(WdfDriverGlobals, obj);
}

// --- IddCx 1.10 wrappers ---

pub unsafe fn idd_cx_device_init_config(
    device_init: PWDFDEVICE_INIT,
    config: *const IDD_CX_CLIENT_CONFIG,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(*mut c_void, PWDFDEVICE_INIT, *const IDD_CX_CLIENT_CONFIG) -> NTSTATUS =
        idd_fn(IDD_DEVICE_INIT_CONFIG);
    f(IddDriverGlobals, device_init, config)
}

pub unsafe fn idd_cx_device_initialize(device: WDFDEVICE) -> NTSTATUS {
    let f: unsafe extern "system" fn(*mut c_void, WDFDEVICE) -> NTSTATUS =
        idd_fn(IDD_DEVICE_INITIALIZE);
    f(IddDriverGlobals, device)
}

pub unsafe fn idd_cx_adapter_init_async(
    in_args: *const IDARG_IN_ADAPTER_INIT,
    out_args: *mut IDARG_OUT_ADAPTER_INIT,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        *const IDARG_IN_ADAPTER_INIT,
        *mut IDARG_OUT_ADAPTER_INIT,
    ) -> NTSTATUS = idd_fn(IDD_ADAPTER_INIT_ASYNC);
    f(IddDriverGlobals, in_args, out_args)
}

pub unsafe fn idd_cx_monitor_create(
    adapter: IDDCX_ADAPTER,
    in_args: *const IDARG_IN_MONITORCREATE,
    out_args: *mut IDARG_OUT_MONITORCREATE,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        IDDCX_ADAPTER,
        *const IDARG_IN_MONITORCREATE,
        *mut IDARG_OUT_MONITORCREATE,
    ) -> NTSTATUS = idd_fn(IDD_MONITOR_CREATE);
    f(IddDriverGlobals, adapter, in_args, out_args)
}

pub unsafe fn idd_cx_monitor_arrival(
    monitor: IDDCX_MONITOR,
    out_args: *mut IDARG_OUT_MONITORARRIVAL,
) -> NTSTATUS {
    let f: unsafe extern "system" fn(
        *mut c_void,
        IDDCX_MONITOR,
        *mut IDARG_OUT_MONITORARRIVAL,
    ) -> NTSTATUS = idd_fn(IDD_MONITOR_ARRIVAL);
    f(IddDriverGlobals, monitor, out_args)
}

pub unsafe fn idd_cx_monitor_departure(monitor: IDDCX_MONITOR) -> NTSTATUS {
    let f: unsafe extern "system" fn(*mut c_void, IDDCX_MONITOR) -> NTSTATUS =
        idd_fn(IDD_MONITOR_DEPARTURE);
    f(IddDriverGlobals, monitor)
}

pub unsafe fn idd_cx_swapchain_set_device(
    swapchain: IDDCX_SWAPCHAIN,
    in_args: *const IDARG_IN_SWAPCHAINSETDEVICE,
) -> HRESULT {
    let f: unsafe extern "system" fn(
        *mut c_void,
        IDDCX_SWAPCHAIN,
        *const IDARG_IN_SWAPCHAINSETDEVICE,
    ) -> HRESULT = idd_fn(IDD_SWAPCHAIN_SET_DEVICE);
    f(IddDriverGlobals, swapchain, in_args)
}

pub unsafe fn idd_cx_swapchain_in_system_memory(
    swapchain: IDDCX_SWAPCHAIN,
    in_system_memory: *mut BOOL,
) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, IDDCX_SWAPCHAIN, *mut BOOL) -> HRESULT =
        idd_fn(IDD_SWAPCHAIN_IN_SYSTEM_MEMORY);
    f(IddDriverGlobals, swapchain, in_system_memory)
}

pub unsafe fn idd_cx_swapchain_release_and_acquire_buffer(
    swapchain: IDDCX_SWAPCHAIN,
    out_args: *mut IDARG_OUT_RELEASEANDACQUIREBUFFER,
) -> HRESULT {
    let f: unsafe extern "system" fn(
        *mut c_void,
        IDDCX_SWAPCHAIN,
        *mut IDARG_OUT_RELEASEANDACQUIREBUFFER,
    ) -> HRESULT = idd_fn(IDD_SWAPCHAIN_RELEASE_AND_ACQUIRE_BUFFER);
    f(IddDriverGlobals, swapchain, out_args)
}

pub unsafe fn idd_cx_swapchain_release_and_acquire_system_buffer(
    swapchain: IDDCX_SWAPCHAIN,
    out_args: *mut IDARG_OUT_RELEASEANDACQUIRESYSTEMBUFFER,
) -> HRESULT {
    let f: unsafe extern "system" fn(
        *mut c_void,
        IDDCX_SWAPCHAIN,
        *mut IDARG_OUT_RELEASEANDACQUIRESYSTEMBUFFER,
    ) -> HRESULT = idd_fn(IDD_SWAPCHAIN_RELEASE_AND_ACQUIRE_SYSTEM_BUFFER);
    f(IddDriverGlobals, swapchain, out_args)
}

pub unsafe fn idd_cx_swapchain_get_dirty_rects(
    swapchain: IDDCX_SWAPCHAIN,
    in_args: *const IDARG_IN_GETDIRTYRECTS,
    out_args: *mut IDARG_OUT_GETDIRTYRECTS,
) -> HRESULT {
    let f: unsafe extern "system" fn(
        *mut c_void,
        IDDCX_SWAPCHAIN,
        *const IDARG_IN_GETDIRTYRECTS,
        *mut IDARG_OUT_GETDIRTYRECTS,
    ) -> HRESULT = idd_fn(IDD_SWAPCHAIN_GET_DIRTY_RECTS);
    f(IddDriverGlobals, swapchain, in_args, out_args)
}

pub unsafe fn idd_cx_swapchain_get_move_regions(
    swapchain: IDDCX_SWAPCHAIN,
    in_args: *const IDARG_IN_GETMOVEREGIONS,
    out_args: *mut IDARG_OUT_GETMOVEREGIONS,
) -> HRESULT {
    let f: unsafe extern "system" fn(
        *mut c_void,
        IDDCX_SWAPCHAIN,
        *const IDARG_IN_GETMOVEREGIONS,
        *mut IDARG_OUT_GETMOVEREGIONS,
    ) -> HRESULT = idd_fn(IDD_SWAPCHAIN_GET_MOVE_REGIONS);
    f(IddDriverGlobals, swapchain, in_args, out_args)
}

pub unsafe fn idd_cx_swapchain_finished_processing_frame(swapchain: IDDCX_SWAPCHAIN) -> HRESULT {
    let f: unsafe extern "system" fn(*mut c_void, IDDCX_SWAPCHAIN) -> HRESULT =
        idd_fn(IDD_SWAPCHAIN_FINISHED_PROCESSING_FRAME);
    f(IddDriverGlobals, swapchain)
}

pub unsafe fn idd_cx_set_realtime_gpu_priority(
    swapchain: IDDCX_SWAPCHAIN,
    in_args: *const IDARG_IN_SETREALTIMEGPUPRIORITY,
) -> HRESULT {
    let f: unsafe extern "system" fn(
        *mut c_void,
        IDDCX_SWAPCHAIN,
        *const IDARG_IN_SETREALTIMEGPUPRIORITY,
    ) -> HRESULT = idd_fn(IDD_SET_REALTIME_GPU_PRIORITY);
    f(IddDriverGlobals, swapchain, in_args)
}
