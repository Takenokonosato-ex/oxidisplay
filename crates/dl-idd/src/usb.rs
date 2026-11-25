//! Zero-leak asynchronous USB 2.0 Bulk OUT transfer pool for DisplayLink DL-1x5.
//!
//! Pre-allocates a fixed ring of 8 x 65,024-byte buffers paired with reusable
//! `WDFREQUEST` + `WDFMEMORY` handles. Never allocates memory during frame streaming,
//! and keeps multiple Bulk OUT transfers in flight simultaneously so USB 2.0
//! High-Speed bandwidth stays saturated while the CPU encodes subsequent chunks.

use core::ffi::c_void;
use core::mem::size_of;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use dl_protocol::{
    encode_blank_mode, encode_mode_set, is_valid_edid, BlankMode, ChunkSink, VideoMode,
    DEFAULT_USB_CHUNK_SIZE, DTU_1031_FALLBACK_EDID, EDID_BLOCK_SIZE, STD_CHANNEL_PAYLOAD,
    USB_REQ_I2C_READ, USB_REQ_SET_CHANNEL,
};

use crate::wdf_iddcx::*;

/// Number of overlapping async Bulk OUT transfers kept in the pre-allocated ring (12 x 128 KiB).
const URB_POOL_SLOTS: usize = 12;
/// USB control/sync timeout in 100ns units (relative negative = 2 seconds).
const USB_TIMEOUT_2S: i64 = -20_000_000;

struct UrbSlot {
    request: WDFREQUEST,
    memory: WDFMEMORY,
    /// Pinned heap buffer referenced by `memory` (`WdfMemoryCreatePreallocated`).
    /// Never reallocated or moved for the lifetime of `UsbTransport`.
    buffer: Box<[u8; DEFAULT_USB_CHUNK_SIZE]>,
}

// WDF handles are thread-safe to submit/reuse under our synchronization discipline.
unsafe impl Send for UrbSlot {}

struct PoolState {
    free_slots: VecDeque<usize>,
    in_flight: usize,
    shutting_down: bool,
}

struct CompletionCtx {
    slot_idx: usize,
    pool: Arc<UrbPoolInner>,
}

struct UrbPoolInner {
    state: Mutex<PoolState>,
    cv: Condvar,
    in_flight_atomic: AtomicUsize,
    io_error_seen: AtomicBool,
    total_bytes_sent: AtomicU64,
    total_usb_errors: AtomicU64,
}

/// Manages WDF USB handles, control transfers (channel init, EDID), modeset, and async bulk ring.
pub struct UsbTransport {
    usb_device: WDFUSBDEVICE,
    bulk_out_pipe: WDFUSBPIPE,
    slots: Vec<UrbSlot>,
    /// Pre-allocated completion context boxes (1 per slot) for pointer stability in WDF callbacks.
    #[allow(clippy::vec_box)]
    ctxs: Vec<Box<CompletionCtx>>,
    pool: Arc<UrbPoolInner>,
}

unsafe impl Send for UsbTransport {}

impl UsbTransport {
    /// Open WDF USB target device, select interface 0, locate Bulk OUT pipe,
    /// initialize standard DisplayLink channel, and pre-allocate the async URB ring.
    pub fn open(wdf_device: WDFDEVICE) -> Result<Self, NTSTATUS> {
        unsafe {
            let mut create_cfg = WDF_USB_DEVICE_CREATE_CONFIG {
                size: size_of::<WDF_USB_DEVICE_CREATE_CONFIG>() as u32,
                usbd_client_contract_version: USBD_CLIENT_CONTRACT_VERSION_602,
            };
            let mut usb_device: WDFUSBDEVICE = core::ptr::null_mut();
            let status = wdf_usb_target_device_create_with_parameters(
                wdf_device,
                &mut create_cfg,
                core::ptr::null_mut(),
                &mut usb_device,
            );
            if !nt_success(status) {
                return Err(status);
            }

            let mut select_params = WDF_USB_DEVICE_SELECT_CONFIG_PARAMS {
                size: size_of::<WDF_USB_DEVICE_SELECT_CONFIG_PARAMS>() as u32,
                config_type: WDF_USB_SELECT_CONFIG_SINGLE_INTERFACE,
                number_configured_pipes: 0,
                _pad: [0; 7],
                configured_usb_interface: core::ptr::null_mut(),
                _reserved: 0,
            };
            let status = wdf_usb_target_device_select_config(
                usb_device,
                core::ptr::null_mut(),
                &mut select_params,
            );
            if !nt_success(status) {
                return Err(status);
            }

            let iface = select_params.configured_usb_interface;
            let num_pipes = select_params.number_configured_pipes;
            let mut bulk_out_pipe: WDFUSBPIPE = core::ptr::null_mut();

            for i in 0..num_pipes {
                let mut info = WDF_USB_PIPE_INFORMATION {
                    size: size_of::<WDF_USB_PIPE_INFORMATION>() as u32,
                    ..Default::default()
                };
                let pipe = wdf_usb_interface_get_configured_pipe(iface, i, &mut info);
                if !pipe.is_null()
                    && info.pipe_type == WDF_USB_PIPE_TYPE_BULK
                    && (info.endpoint_address & 0x80) == 0
                {
                    bulk_out_pipe = pipe;
                    break;
                }
            }

            if bulk_out_pipe.is_null() {
                return Err(STATUS_DEVICE_NOT_CONNECTED);
            }

            wdf_usb_target_pipe_set_no_max_packet_check(bulk_out_pipe);

            // Pre-allocate the fixed ring of async URB slots.
            let mut slots = Vec::with_capacity(URB_POOL_SLOTS);
            let mut free_slots = VecDeque::with_capacity(URB_POOL_SLOTS);
            let pool = Arc::new(UrbPoolInner {
                state: Mutex::new(PoolState {
                    free_slots: VecDeque::new(),
                    in_flight: 0,
                    shutting_down: false,
                }),
                cv: Condvar::new(),
                in_flight_atomic: AtomicUsize::new(0),
                io_error_seen: AtomicBool::new(false),
                total_bytes_sent: AtomicU64::new(0),
                total_usb_errors: AtomicU64::new(0),
            });
            let mut ctxs = Vec::with_capacity(URB_POOL_SLOTS);

            for idx in 0..URB_POOL_SLOTS {
                let mut buffer = Box::new([0u8; DEFAULT_USB_CHUNK_SIZE]);
                let mut req_attrs = WDF_OBJECT_ATTRIBUTES::init(wdf_device);
                let mut request: WDFREQUEST = core::ptr::null_mut();
                let st = wdf_request_create(&mut req_attrs, bulk_out_pipe, &mut request);
                if !nt_success(st) {
                    Self::cleanup_partial_slots(&mut slots);
                    return Err(st);
                }

                let mut mem_attrs = WDF_OBJECT_ATTRIBUTES::init(request);
                let mut memory: WDFMEMORY = core::ptr::null_mut();
                let st = wdf_memory_create_preallocated(
                    &mut mem_attrs,
                    buffer.as_mut_ptr() as *mut c_void,
                    DEFAULT_USB_CHUNK_SIZE,
                    &mut memory,
                );
                if !nt_success(st) {
                    wdf_object_delete(request);
                    Self::cleanup_partial_slots(&mut slots);
                    return Err(st);
                }

                slots.push(UrbSlot {
                    request,
                    memory,
                    buffer,
                });
                ctxs.push(Box::new(CompletionCtx {
                    slot_idx: idx,
                    pool: Arc::clone(&pool),
                }));
                free_slots.push_back(idx);
            }
            pool.state.lock().unwrap().free_slots = free_slots;

            let transport = Self {
                usb_device,
                bulk_out_pipe,
                slots,
                ctxs,
                pool,
            };

            // Initialize DisplayLink standard channel (control OUT 0x12).
            let _ = transport.select_std_channel();

            Ok(transport)
        }
    }

    unsafe fn cleanup_partial_slots(slots: &mut Vec<UrbSlot>) {
        for slot in slots.drain(..) {
            wdf_object_delete(slot.request);
        }
    }

    /// Send DisplayLink standard channel initialization control request (0x12).
    pub fn select_std_channel(&self) -> Result<(), NTSTATUS> {
        let mut payload = STD_CHANNEL_PAYLOAD;
        let mut setup = WDF_USB_CONTROL_SETUP_PACKET {
            bm_request_type: 0x40, // Host-to-device | Vendor | Device
            b_request: USB_REQ_SET_CHANNEL,
            w_value: 0,
            w_index: 0,
            w_length: payload.len() as u16,
        };
        let mut mem = WDF_MEMORY_DESCRIPTOR {
            desc_type: WDF_MEMORY_DESCRIPTOR_TYPE_BUFFER,
            _pad0: 0,
            buffer: payload.as_mut_ptr() as *mut c_void,
            length: payload.len() as u32,
            _pad1: 0,
        };
        let mut opts = WDF_REQUEST_SEND_OPTIONS {
            size: size_of::<WDF_REQUEST_SEND_OPTIONS>() as u32,
            flags: WDF_REQUEST_SEND_OPTION_TIMEOUT,
            timeout: USB_TIMEOUT_2S,
        };
        let mut transferred = 0u32;
        let st = unsafe {
            wdf_usb_target_device_send_control_sync(
                self.usb_device,
                core::ptr::null_mut(),
                &mut opts,
                &mut setup,
                &mut mem,
                &mut transferred,
            )
        };
        if nt_success(st) {
            Ok(())
        } else {
            Err(st)
        }
    }

    /// Read the 128-byte monitor EDID over DisplayLink vendor I2C control IN (0x02).
    /// Falls back to the verified DTU-1031 EDID if the hardware read fails or fails checksum.
    pub fn read_edid_or_fallback(&self) -> [u8; EDID_BLOCK_SIZE] {
        let mut edid = [0u8; EDID_BLOCK_SIZE];
        let mut ok = true;

        for (i, dst_byte) in edid.iter_mut().enumerate() {
            let mut rbuf = [0u8; 2];
            let mut setup = WDF_USB_CONTROL_SETUP_PACKET {
                bm_request_type: 0xC0, // Device-to-host | Vendor | Device
                b_request: USB_REQ_I2C_READ,
                w_value: ((i as u16) << 8),
                w_index: 0x00A1,
                w_length: 2,
            };
            let mut mem = WDF_MEMORY_DESCRIPTOR {
                desc_type: WDF_MEMORY_DESCRIPTOR_TYPE_BUFFER,
                _pad0: 0,
                buffer: rbuf.as_mut_ptr() as *mut c_void,
                length: 2,
                _pad1: 0,
            };
            let mut opts = WDF_REQUEST_SEND_OPTIONS {
                size: size_of::<WDF_REQUEST_SEND_OPTIONS>() as u32,
                flags: WDF_REQUEST_SEND_OPTION_TIMEOUT,
                timeout: USB_TIMEOUT_2S,
            };
            let mut transferred = 0u32;
            let st = unsafe {
                wdf_usb_target_device_send_control_sync(
                    self.usb_device,
                    core::ptr::null_mut(),
                    &mut opts,
                    &mut setup,
                    &mut mem,
                    &mut transferred,
                )
            };
            if !nt_success(st) || transferred < 2 {
                ok = false;
                break;
            }
            *dst_byte = rbuf[1];
        }

        if ok && is_valid_edid(&edid) {
            edid
        } else {
            DTU_1031_FALLBACK_EDID
        }
    }

    /// Program the DL-1x5 display controller with `mode` (synchronous after draining in-flight URBs).
    pub fn set_video_mode(&mut self, mode: &VideoMode) -> Result<(), NTSTATUS> {
        self.wait_all_in_flight(Duration::from_secs(2));
        let mut cmd = Vec::with_capacity(256);
        encode_mode_set(mode, &mut cmd);
        self.write_sync(&cmd)
    }

    /// Set blank / DPMS power state on the DL-1x5 hardware.
    pub fn set_blank_mode(&mut self, blank: BlankMode) -> Result<(), NTSTATUS> {
        self.wait_all_in_flight(Duration::from_secs(1));
        let mut cmd = Vec::with_capacity(64);
        encode_blank_mode(blank, &mut cmd);
        self.write_sync(&cmd)
    }

    /// Synchronous small bulk write (used only for infrequent modeset/blank commands).
    fn write_sync(&self, data: &[u8]) -> Result<(), NTSTATUS> {
        if data.is_empty() {
            return Ok(());
        }
        let mut mem = WDF_MEMORY_DESCRIPTOR {
            desc_type: WDF_MEMORY_DESCRIPTOR_TYPE_BUFFER,
            _pad0: 0,
            buffer: data.as_ptr() as *mut c_void,
            length: data.len() as u32,
            _pad1: 0,
        };
        let mut opts = WDF_REQUEST_SEND_OPTIONS {
            size: size_of::<WDF_REQUEST_SEND_OPTIONS>() as u32,
            flags: WDF_REQUEST_SEND_OPTION_TIMEOUT,
            timeout: USB_TIMEOUT_2S,
        };
        let mut written = 0u32;
        let st = unsafe {
            wdf_usb_target_pipe_write_sync(
                self.bulk_out_pipe,
                core::ptr::null_mut(),
                &mut opts,
                &mut mem,
                &mut written,
            )
        };
        if nt_success(st) {
            self.pool
                .total_bytes_sent
                .fetch_add(written as u64, Ordering::Relaxed);
            Ok(())
        } else {
            self.pool.total_usb_errors.fetch_add(1, Ordering::Relaxed);
            Err(st)
        }
    }

    /// Reset Bulk OUT pipe after a transient USB error and re-select standard channel.
    pub fn recover_pipe(&mut self) -> bool {
        self.wait_all_in_flight(Duration::from_secs(1));
        let mut opts = WDF_REQUEST_SEND_OPTIONS {
            size: size_of::<WDF_REQUEST_SEND_OPTIONS>() as u32,
            flags: WDF_REQUEST_SEND_OPTION_TIMEOUT,
            timeout: USB_TIMEOUT_2S,
        };
        let st = unsafe {
            wdf_usb_target_pipe_reset_sync(
                self.bulk_out_pipe,
                core::ptr::null_mut(),
                &mut opts,
            )
        };
        if nt_success(st) {
            self.pool.io_error_seen.store(false, Ordering::Release);
            let _ = self.select_std_channel();
            true
        } else {
            false
        }
    }

    pub fn take_io_error(&self) -> bool {
        self.pool.io_error_seen.swap(false, Ordering::AcqRel)
    }

    #[inline]
    pub fn in_flight_slots(&self) -> usize {
        self.pool.in_flight_atomic.load(Ordering::Relaxed)
    }

    #[allow(dead_code)]
    pub fn stats(&self) -> (u64, u64) {
        (
            self.pool.total_bytes_sent.load(Ordering::Relaxed),
            self.pool.total_usb_errors.load(Ordering::Relaxed),
        )
    }

    /// Create a zero-allocation `ChunkSink` borrowing this transport's async URB ring for one frame.
    pub fn async_sink(&mut self) -> AsyncUsbSink<'_> {
        AsyncUsbSink {
            transport: self,
            active_slot: None,
        }
    }

    /// Wait until all currently in-flight async Bulk OUT transfers finish.
    pub fn wait_all_in_flight(&self, timeout: Duration) {
        let mut guard = self.pool.state.lock().unwrap();
        let deadline = std::time::Instant::now() + timeout;
        while guard.in_flight > 0 {
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            let (g, res) = self.pool.cv.wait_timeout(guard, deadline - now).unwrap();
            guard = g;
            if res.timed_out() {
                break;
            }
        }
    }

    /// Cancel all in-flight transfers and drain before device removal or D0Exit.
    pub fn shutdown(&mut self) {
        {
            let mut guard = self.pool.state.lock().unwrap();
            guard.shutting_down = true;
            self.pool.cv.notify_all();
        }
        unsafe {
            wdf_io_target_stop(self.bulk_out_pipe, WDF_IO_TARGET_CANCEL_SENT_IO);
        }
        self.wait_all_in_flight(Duration::from_secs(2));
    }

    /// Re-enable I/O target on D0Entry after a previous D0Exit stop.
    pub fn restart(&mut self) {
        {
            let mut guard = self.pool.state.lock().unwrap();
            guard.shutting_down = false;
        }
        unsafe {
            let _ = wdf_io_target_start(self.bulk_out_pipe);
        }
        let _ = self.select_std_channel();
    }
}

impl Drop for UsbTransport {
    fn drop(&mut self) {
        self.shutdown();
        unsafe {
            for slot in self.slots.drain(..) {
                // Deleting the request also deletes its child WDFMEMORY before `slot.buffer` drops.
                wdf_object_delete(slot.request);
            }
        }
    }
}

/// Async zero-copy `ChunkSink` backed by `UsbTransport`'s pre-allocated `WDFREQUEST` ring.
pub struct AsyncUsbSink<'a> {
    transport: &'a mut UsbTransport,
    active_slot: Option<usize>,
}

impl<'a> ChunkSink for AsyncUsbSink<'a> {
    type Error = NTSTATUS;

    fn acquire_chunk(&mut self) -> Result<&mut [u8], Self::Error> {
        let slot_idx = match self.active_slot {
            Some(idx) => idx,
            None => {
                let mut guard = self.transport.pool.state.lock().unwrap();
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                loop {
                    if guard.shutting_down {
                        return Err(STATUS_DEVICE_NOT_CONNECTED);
                    }
                    if let Some(idx) = guard.free_slots.pop_front() {
                        self.active_slot = Some(idx);
                        break idx;
                    }
                    let now = std::time::Instant::now();
                    if now >= deadline {
                        return Err(STATUS_UNSUCCESSFUL);
                    }
                    let (g, _) = self
                        .transport
                        .pool
                        .cv
                        .wait_timeout(guard, deadline - now)
                        .unwrap();
                    guard = g;
                }
            }
        };
        Ok(&mut self.transport.slots[slot_idx].buffer[..])
    }

    fn submit_chunk(&mut self, len: usize) -> Result<(), Self::Error> {
        let slot_idx = match self.active_slot.take() {
            Some(idx) => idx,
            None => return Ok(()),
        };
        if len == 0 {
            let mut guard = self.transport.pool.state.lock().unwrap();
            guard.free_slots.push_back(slot_idx);
            self.transport.pool.cv.notify_one();
            return Ok(());
        }

        let len = len.min(DEFAULT_USB_CHUNK_SIZE);
        let slot = &self.transport.slots[slot_idx];
        let ctx_ptr = (&*self.transport.ctxs[slot_idx]) as *const CompletionCtx as *mut c_void;

        unsafe {
            let mut reuse = WDF_REQUEST_REUSE_PARAMS {
                size: size_of::<WDF_REQUEST_REUSE_PARAMS>() as u32,
                flags: 0,
                status: STATUS_SUCCESS,
                _pad0: 0,
                new_irp: core::ptr::null_mut(),
            };
            let st = wdf_request_reuse(slot.request, &mut reuse);
            if !nt_success(st) {
                self.return_slot(slot_idx);
                return Err(st);
            }

            let mut offset = WDFMEMORY_OFFSET {
                buffer_offset: 0,
                buffer_length: len,
            };
            let st = wdf_usb_target_pipe_format_request_for_write(
                self.transport.bulk_out_pipe,
                slot.request,
                slot.memory,
                &mut offset,
            );
            if !nt_success(st) {
                self.return_slot(slot_idx);
                return Err(st);
            }

            wdf_request_set_completion_routine(
                slot.request,
                Some(urb_completion_routine),
                ctx_ptr,
            );

            {
                let mut guard = self.transport.pool.state.lock().unwrap();
                guard.in_flight += 1;
                self.transport
                    .pool
                    .in_flight_atomic
                    .store(guard.in_flight, Ordering::Relaxed);
            }

            let sent = wdf_request_send(
                slot.request,
                self.transport.bulk_out_pipe,
                core::ptr::null_mut(),
            );
            if sent == 0 {
                {
                    let mut guard = self.transport.pool.state.lock().unwrap();
                    guard.in_flight = guard.in_flight.saturating_sub(1);
                    self.transport
                        .pool
                        .in_flight_atomic
                        .store(guard.in_flight, Ordering::Relaxed);
                    guard.free_slots.push_back(slot_idx);
                    self.transport.pool.cv.notify_all();
                }
                self.transport.pool.io_error_seen.store(true, Ordering::Release);
                self.transport
                    .pool
                    .total_usb_errors
                    .fetch_add(1, Ordering::Relaxed);
                return Err(STATUS_UNSUCCESSFUL);
            }
        }
        Ok(())
    }
}

impl<'a> AsyncUsbSink<'a> {
    fn return_slot(&mut self, slot_idx: usize) {
        let mut guard = self.transport.pool.state.lock().unwrap();
        guard.free_slots.push_back(slot_idx);
        self.transport.pool.cv.notify_one();
    }
}

impl<'a> Drop for AsyncUsbSink<'a> {
    fn drop(&mut self) {
        if let Some(slot_idx) = self.active_slot.take() {
            self.return_slot(slot_idx);
        }
    }
}

unsafe extern "system" fn urb_completion_routine(
    _request: WDFREQUEST,
    _target: WDFIOTARGET,
    params: *mut WDF_REQUEST_COMPLETION_PARAMS,
    context: *mut c_void,
) {
    if context.is_null() {
        return;
    }
    let ctx = &*(context as *const CompletionCtx);
    if !params.is_null() {
        let status = (*params).io_status_status;
        if nt_success(status) {
            ctx.pool
                .total_bytes_sent
                .fetch_add((*params).io_status_info as u64, Ordering::Relaxed);
        } else {
            ctx.pool.io_error_seen.store(true, Ordering::Release);
            ctx.pool.total_usb_errors.fetch_add(1, Ordering::Relaxed);
        }
    }
    let mut guard = ctx.pool.state.lock().unwrap();
    guard.in_flight = guard.in_flight.saturating_sub(1);
    ctx.pool
        .in_flight_atomic
        .store(guard.in_flight, Ordering::Relaxed);
    guard.free_slots.push_back(ctx.slot_idx);
    ctx.pool.cv.notify_all();
}
