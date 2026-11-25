//! Dedicated 60 FPS real-time swapchain frame pump.
//!
//! Zero per-frame heap allocations:
//! - Reuses pre-allocated `FrameEncoder` (shadow RGB565 buffer + 1-row scratch)
//! - Reuses pre-allocated `dirty_rects` & `move_regions` scratch buffers
//! - Streams directly into `UsbTransport`'s 8-slot async Bulk OUT URB ring
//! - Supports both IddCx 1.10 zero-copy `SystemBuffer` and D3D11 staging readback

use core::ffi::c_void;
use core::mem::size_of;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use dl_protocol::{DirtyRect, FrameEncoder, MoveRegion, QualityTier, VideoMode};

use crate::d3d::RenderDevice;
use crate::usb::UsbTransport;
use crate::wdf_iddcx::*;

const WAIT_OBJECT_0: u32 = 0;
const WAIT_TIMEOUT: u32 = 258;
const THREAD_PRIORITY_HIGHEST: i32 = 2;

extern "system" {
    fn CreateEventW(
        lp_event_attributes: *mut c_void,
        b_manual_reset: BOOL,
        b_initial_state: BOOL,
        lp_name: *const u16,
    ) -> HANDLE;
    fn SetEvent(h_event: HANDLE) -> BOOL;
    fn CloseHandle(h_object: HANDLE) -> BOOL;
    fn WaitForSingleObject(h_handle: HANDLE, dw_milliseconds: u32) -> u32;
    fn WaitForMultipleObjects(
        n_count: u32,
        lp_handles: *const HANDLE,
        b_wait_all: BOOL,
        dw_milliseconds: u32,
    ) -> u32;
    fn GetCurrentThread() -> HANDLE;
    fn SetThreadPriority(h_thread: HANDLE, n_priority: i32) -> BOOL;
    fn AvSetMmThreadCharacteristicsW(task_name: *const u16, task_index: *mut u32) -> HANDLE;
    fn AvRevertMmThreadCharacteristics(avrt_handle: HANDLE) -> BOOL;
}

/// Handle to the running frame-processing thread for a monitor's active swapchain.
pub struct SwapChainProcessor {
    swapchain: IDDCX_SWAPCHAIN,
    stop_event: HANDLE,
    thread: Option<JoinHandle<()>>,
}

unsafe impl Send for SwapChainProcessor {}

impl SwapChainProcessor {
    pub fn start(
        swapchain: IDDCX_SWAPCHAIN,
        available_event: HANDLE,
        render_adapter_luid: LUID,
        mode: VideoMode,
        usb: Arc<Mutex<Option<UsbTransport>>>,
    ) -> Result<Self, NTSTATUS> {
        let stop_event = unsafe { CreateEventW(core::ptr::null_mut(), 1, 0, core::ptr::null()) };
        if stop_event.is_null() {
            return Err(STATUS_NO_MEMORY);
        }

        let sc_usize = swapchain as usize;
        let avail_usize = available_event as usize;
        let stop_usize = stop_event as usize;

        let handle = thread::Builder::new()
            .name("dl-rust-60fps".to_string())
            .spawn(move || {
                run_frame_loop(
                    sc_usize as IDDCX_SWAPCHAIN,
                    avail_usize as HANDLE,
                    stop_usize as HANDLE,
                    render_adapter_luid,
                    mode,
                    usb,
                );
            })
            .map_err(|_| {
                unsafe { CloseHandle(stop_event) };
                STATUS_NO_MEMORY
            })?;

        Ok(Self {
            swapchain,
            stop_event,
            thread: Some(handle),
        })
    }

    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        if !self.stop_event.is_null() {
            unsafe { SetEvent(self.stop_event) };
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if !self.stop_event.is_null() {
            unsafe { CloseHandle(self.stop_event) };
            self.stop_event = core::ptr::null_mut();
        }
        // Per IddCx spec, driver owns the IDDCX_SWAPCHAIN WDF object and must delete it on unassign.
        if !self.swapchain.is_null() {
            unsafe { wdf_object_delete(self.swapchain) };
            self.swapchain = core::ptr::null_mut();
        }
    }
}

impl Drop for SwapChainProcessor {
    fn drop(&mut self) {
        self.stop_inner();
    }
}

fn run_frame_loop(
    swapchain: IDDCX_SWAPCHAIN,
    available_event: HANDLE,
    stop_event: HANDLE,
    render_adapter_luid: LUID,
    mode: VideoMode,
    usb_lock: Arc<Mutex<Option<UsbTransport>>>,
) {
    // Elevate thread priority via MMCSS ("DisplayPostProcessing") for smooth 60 FPS pacing.
    let task_name: [u16; 22] = [
        b'D' as u16, b'i' as u16, b's' as u16, b'p' as u16, b'l' as u16, b'a' as u16, b'y' as u16,
        b'P' as u16, b'o' as u16, b's' as u16, b't' as u16, b'P' as u16, b'r' as u16, b'o' as u16,
        b'c' as u16, b'e' as u16, b's' as u16, b's' as u16, b'i' as u16, b'n' as u16, b'g' as u16,
        0,
    ];
    let mut task_idx = 0u32;
    let mmcss = unsafe { AvSetMmThreadCharacteristicsW(task_name.as_ptr(), &mut task_idx) };
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) };

    // Initialize D3D11 device on DWM's render adapter and bind it to the swapchain.
    let mut render_dev = match RenderDevice::new(render_adapter_luid) {
        Ok(d) => d,
        Err(hr) => {
            crate::trace_log(&format!("SwapChain: RenderDevice::new FAILED hr=0x{:08X}", hr as u32));
            if !mmcss.is_null() {
                unsafe { AvRevertMmThreadCharacteristics(mmcss) };
            }
            return;
        }
    };

    let set_dev = IDARG_IN_SWAPCHAINSETDEVICE {
        device: render_dev.dxgi_device_ptr(),
    };
    let hr = unsafe { idd_cx_swapchain_set_device(swapchain, &set_dev) };
    if !succeeded(hr) {
        crate::trace_log(&format!("SwapChain: SetDevice FAILED hr=0x{:08X}", hr as u32));
        if !mmcss.is_null() {
            unsafe { AvRevertMmThreadCharacteristics(mmcss) };
        }
        return;
    }

    // Request realtime GPU priority so primary-GPU workloads never stall our staging copies.
    let rt_prio = IDARG_IN_SETREALTIMEGPUPRIORITY {
        device: render_dev.dxgi_device_ptr(),
    };
    let _ = unsafe { idd_cx_set_realtime_gpu_priority(swapchain, &rt_prio) };

    // Check if DWM placed this swapchain directly in CPU-accessible system memory (IddCx 1.10).
    let mut in_sysmem: BOOL = 0;
    let use_sysmem = unsafe {
        succeeded(idd_cx_swapchain_in_system_memory(swapchain, &mut in_sysmem)) && in_sysmem != 0
    };
    crate::trace_log(&format!(
        "SwapChain: started {}x{} use_sysmem={}",
        mode.hdisplay, mode.vdisplay, use_sysmem
    ));

    // Pre-allocate ALL per-swapchain buffers ONCE outside the loop (0 bytes allocated per frame!).
    let mut encoder = FrameEncoder::new(mode.hdisplay as u32, mode.vdisplay as u32);
    let mut raw_dirty_rects: Vec<RECT> = vec![RECT::default(); 64];
    let mut raw_move_regions: Vec<IDDCX_MOVEREGION> = vec![IDDCX_MOVEREGION::default(); 32];
    // Two pre-allocated region pairs matching our ping-pong staging textures [0] and [1].
    let mut slot_dirty: [Vec<DirtyRect>; 2] = [Vec::with_capacity(64), Vec::with_capacity(64)];
    let mut slot_moves: [Vec<MoveRegion>; 2] = [Vec::with_capacity(32), Vec::with_capacity(32)];
    let mut pending_slot: Option<usize> = None;
    let mut next_write_slot: usize = 0;

    // Ensure hardware display controller is configured to the active mode.
    if let Ok(mut guard) = usb_lock.lock() {
        if let Some(ref mut usb) = *guard {
            let st = usb.set_video_mode(&mode);
            crate::trace_log(&format!("SwapChain: set_video_mode={:?}", st));
        }
    }

    let handles = [stop_event, available_event];
    let mut frame_idx: u64 = 0;
    let mut ewma_frame_bytes: f64 = 0.0;
    let mut current_tier = QualityTier::Lossless;
    let mut stats_window_start = std::time::Instant::now();
    let mut stats_window_frames: u64 = 0;
    let mut stats_window_bytes: u64 = 0;

    loop {
        if unsafe { WaitForSingleObject(stop_event, 0) } == WAIT_OBJECT_0 {
            crate::trace_log("SwapChain: stop_event signaled");
            break;
        }

        if use_sysmem {
            let mut meta: IDDCX_METADATA = unsafe { core::mem::zeroed() };
            meta.size = size_of::<IDDCX_METADATA>() as u32;
            let mut buf_info: IDDCX_SYSTEM_BUFFER_INFO = unsafe { core::mem::zeroed() };
            buf_info.size = size_of::<IDDCX_SYSTEM_BUFFER_INFO>() as u32;
            let mut out = IDARG_OUT_RELEASEANDACQUIRESYSTEMBUFFER {
                meta_data: &mut meta,
                buffer_info: &mut buf_info,
            };

            let hr = unsafe { idd_cx_swapchain_release_and_acquire_system_buffer(swapchain, &mut out) };
            if hr == E_PENDING {
                let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, 100) };
                if wait == WAIT_OBJECT_0 {
                    break;
                }
                if wait != WAIT_OBJECT_0 + 1 && wait != WAIT_TIMEOUT {
                    break;
                }
                continue;
            }
            if !succeeded(hr) {
                crate::trace_log(&format!("SwapChain: AcquireSysBuf failed hr=0x{:08X}", hr as u32));
                break;
            }

            collect_regions(
                swapchain,
                &meta,
                buf_info.width,
                buf_info.height,
                &mut raw_dirty_rects,
                &mut raw_move_regions,
                &mut slot_dirty[0],
                &mut slot_moves[0],
            );

            let mut frame_bytes = 0u64;
            if !buf_info.buffer.is_null() && buf_info.width > 0 && buf_info.height > 0 {
                encoder.resize(buf_info.width, buf_info.height);
                let total_bytes = (buf_info.pitch as usize).saturating_mul(buf_info.height as usize);
                let surface = unsafe {
                    core::slice::from_raw_parts(buf_info.buffer as *const u8, total_bytes)
                };
                let (bytes, tier) = submit_encoded_frame(
                    &mut encoder,
                    surface,
                    buf_info.pitch as usize,
                    &slot_moves[0],
                    &slot_dirty[0],
                    &mode,
                    &usb_lock,
                    ewma_frame_bytes,
                    current_tier,
                    frame_idx,
                );
                frame_bytes = bytes;
                current_tier = tier;
            }

            let _ = unsafe { idd_cx_swapchain_finished_processing_frame(swapchain) };
            ewma_frame_bytes = ewma_frame_bytes * 0.75 + (frame_bytes as f64) * 0.25;
            frame_idx += 1;
            stats_window_frames += 1;
            stats_window_bytes += frame_bytes;
        } else {
            let mut out: IDARG_OUT_RELEASEANDACQUIREBUFFER = unsafe { core::mem::zeroed() };
            out.meta_data.size = size_of::<IDDCX_METADATA>() as u32;

            let hr = unsafe { idd_cx_swapchain_release_and_acquire_buffer(swapchain, &mut out) };
            if hr == E_PENDING {
                // If a previous frame is sitting in our ping-pong staging buffer, wait briefly (6 ms)
                // and flush it immediately on timeout so idle screens never lag by 1 frame!
                let wait_ms = if pending_slot.is_some() { 6 } else { 100 };
                let wait = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, wait_ms) };
                if wait == WAIT_OBJECT_0 {
                    break;
                }
                if wait == WAIT_TIMEOUT {
                    if let Some(slot) = pending_slot.take() {
                        if let Ok((b, tier)) = render_dev.map_staging(slot, |slice, stride, w, h| {
                            encoder.resize(w, h);
                            submit_encoded_frame(
                                &mut encoder,
                                slice,
                                stride,
                                &slot_moves[slot],
                                &slot_dirty[slot],
                                &mode,
                                &usb_lock,
                                ewma_frame_bytes,
                                current_tier,
                                frame_idx,
                            )
                        }) {
                            ewma_frame_bytes = ewma_frame_bytes * 0.75 + (b as f64) * 0.25;
                            current_tier = tier;
                            frame_idx += 1;
                            stats_window_frames += 1;
                            stats_window_bytes += b;
                        }
                    }
                    continue;
                }
                if wait != WAIT_OBJECT_0 + 1 {
                    break;
                }
                continue;
            }
            if !succeeded(hr) {
                crate::trace_log(&format!("SwapChain: AcquireBuf failed hr=0x{:08X}", hr as u32));
                break;
            }

            let meta = out.meta_data;
            let surface_ptr = meta.surface;
            let mut finished_called = false;

            if !surface_ptr.is_null() {
                let write_slot = next_write_slot;
                collect_regions(
                    swapchain,
                    &meta,
                    encoder.width(),
                    encoder.height(),
                    &mut raw_dirty_rects,
                    &mut raw_move_regions,
                    &mut slot_dirty[write_slot],
                    &mut slot_moves[write_slot],
                );

                let is_idle_reencode =
                    slot_dirty[write_slot].is_empty() && slot_moves[write_slot].is_empty() && frame_idx > 0;

                if is_idle_reencode && !encoder.needs_refinement() && pending_slot.is_none() {
                    let _ = unsafe { idd_cx_swapchain_finished_processing_frame(swapchain) };
                    finished_called = true;
                } else {
                    let need_full_copy = frame_idx == 0 || is_idle_reencode;
                    let copy_res = render_dev.copy_to_staging(
                        write_slot,
                        surface_ptr,
                        &slot_dirty[write_slot],
                        need_full_copy,
                        || {
                            let _ = unsafe { idd_cx_swapchain_finished_processing_frame(swapchain) };
                            finished_called = true;
                        },
                    );

                    if copy_res.is_ok() {
                        // Ping-pong: encode the PREVIOUS slot (already finished PCIe DMA -> 0.0 ms Map stall!)
                        // while the GPU executes `copy_to_staging(write_slot)` asynchronously in parallel!
                        if let Some(read_slot) = pending_slot.replace(write_slot) {
                            if let Ok((b, tier)) = render_dev.map_staging(read_slot, |slice, stride, w, h| {
                                encoder.resize(w, h);
                                submit_encoded_frame(
                                    &mut encoder,
                                    slice,
                                    stride,
                                    &slot_moves[read_slot],
                                    &slot_dirty[read_slot],
                                    &mode,
                                    &usb_lock,
                                    ewma_frame_bytes,
                                    current_tier,
                                    frame_idx,
                                )
                            }) {
                                ewma_frame_bytes = ewma_frame_bytes * 0.75 + (b as f64) * 0.25;
                                current_tier = tier;
                                frame_idx += 1;
                                stats_window_frames += 1;
                                stats_window_bytes += b;
                            }
                        }
                        next_write_slot ^= 1;
                    }
                }
            }

            if !finished_called {
                let _ = unsafe { idd_cx_swapchain_finished_processing_frame(swapchain) };
            }
        }

        // Periodic lightweight telemetry every 5 seconds.
        let elapsed = stats_window_start.elapsed().as_secs_f64();
        if elapsed >= 5.0 {
            let fps = stats_window_frames as f64 / elapsed;
            let kbps = (stats_window_bytes as f64 / 1024.0) / elapsed;
            crate::trace_log(&format!(
                "Telemetry: total_frames={frame_idx} fps={fps:.1} usb_rate={kbps:.1} KB/s tier={current_tier:?}"
            ));
            stats_window_start = std::time::Instant::now();
            stats_window_frames = 0;
            stats_window_bytes = 0;
        }
    }

    if !mmcss.is_null() {
        unsafe { AvRevertMmThreadCharacteristics(mmcss) };
    }
}

#[allow(clippy::too_many_arguments)]
fn collect_regions(
    swapchain: IDDCX_SWAPCHAIN,
    meta: &IDDCX_METADATA,
    width: u32,
    height: u32,
    raw_dirty: &mut Vec<RECT>,
    raw_moves: &mut Vec<IDDCX_MOVEREGION>,
    dirty_out: &mut Vec<DirtyRect>,
    moves_out: &mut Vec<MoveRegion>,
) {
    dirty_out.clear();
    moves_out.clear();

    if meta.move_region_count > 0 {
        let count = (meta.move_region_count as usize).min(128);
        if raw_moves.len() < count {
            raw_moves.resize(count, IDDCX_MOVEREGION::default());
        }
        let in_args = IDARG_IN_GETMOVEREGIONS {
            move_region_in_count: count as u32,
            _pad0: 0,
            move_regions: raw_moves.as_mut_ptr(),
        };
        let mut out_args = IDARG_OUT_GETMOVEREGIONS {
            move_region_out_count: 0,
        };
        if unsafe { succeeded(idd_cx_swapchain_get_move_regions(swapchain, &in_args, &mut out_args)) }
        {
            for m in &raw_moves[..out_args.move_region_out_count as usize] {
                if m.source_point.x >= 0
                    && m.source_point.y >= 0
                    && m.dest_rect.left >= 0
                    && m.dest_rect.top >= 0
                    && m.dest_rect.right > m.dest_rect.left
                    && m.dest_rect.bottom > m.dest_rect.top
                {
                    moves_out.push(MoveRegion {
                        src_x: m.source_point.x as u32,
                        src_y: m.source_point.y as u32,
                        dst_x: m.dest_rect.left as u32,
                        dst_y: m.dest_rect.top as u32,
                        width: (m.dest_rect.right - m.dest_rect.left) as u32,
                        height: (m.dest_rect.bottom - m.dest_rect.top) as u32,
                    });
                }
            }
        }
    }

    if meta.dirty_rect_count > 0 {
        let count = (meta.dirty_rect_count as usize).min(256);
        if raw_dirty.len() < count {
            raw_dirty.resize(count, RECT::default());
        }
        let in_args = IDARG_IN_GETDIRTYRECTS {
            dirty_rect_in_count: count as u32,
            _pad0: 0,
            dirty_rects: raw_dirty.as_mut_ptr(),
        };
        let mut out_args = IDARG_OUT_GETDIRTYRECTS {
            dirty_rect_out_count: 0,
        };
        if unsafe { succeeded(idd_cx_swapchain_get_dirty_rects(swapchain, &in_args, &mut out_args)) }
        {
            for r in &raw_dirty[..out_args.dirty_rect_out_count as usize] {
                if r.right > r.left && r.bottom > r.top {
                    dirty_out.push(DirtyRect {
                        left: r.left.max(0) as u32,
                        top: r.top.max(0) as u32,
                        right: (r.right as u32).min(width),
                        bottom: (r.bottom as u32).min(height),
                    });
                }
            }
        }
    }
}

fn choose_quality_tier(in_flight: usize, ewma_bytes: f64, prev: QualityTier) -> QualityTier {
    // Hysteresis prevents rapid oscillation between quality tiers.
    if in_flight >= 5 || ewma_bytes > 460_000.0 {
        QualityTier::MotionUltra
    } else if in_flight >= 2 || ewma_bytes > 260_000.0 {
        if prev == QualityTier::MotionUltra && (in_flight >= 3 || ewma_bytes > 360_000.0) {
            QualityTier::MotionUltra
        } else {
            QualityTier::MotionFast
        }
    } else if prev == QualityTier::MotionFast && ewma_bytes > 180_000.0 {
        QualityTier::MotionFast
    } else {
        QualityTier::Lossless
    }
}

#[allow(clippy::too_many_arguments)]
fn submit_encoded_frame(
    encoder: &mut FrameEncoder,
    surface: &[u8],
    stride: usize,
    move_regions: &[MoveRegion],
    dirty_rects: &[DirtyRect],
    mode: &VideoMode,
    usb_lock: &Arc<Mutex<Option<UsbTransport>>>,
    ewma_bytes: f64,
    prev_tier: QualityTier,
    frame_idx: u64,
) -> (u64, QualityTier) {
    let Ok(mut guard) = usb_lock.lock() else {
        return (0, prev_tier);
    };
    let Some(ref mut usb) = *guard else {
        return (0, prev_tier);
    };

    if usb.take_io_error() {
        crate::trace_log("SwapChain: USB io_error_seen -> recovering pipe");
        if usb.recover_pipe() {
            let _ = usb.set_video_mode(mode);
            encoder.invalidate_shadow();
        }
    }

    let in_flight = usb.in_flight_slots();
    let tier = choose_quality_tier(in_flight, ewma_bytes, prev_tier);

    let mut sink = usb.async_sink();
    match encoder.encode_frame_bgra_adaptive(surface, stride, move_regions, dirty_rects, tier, &mut sink) {
        Ok(stats) => {
            if frame_idx < 3 {
                crate::trace_log(&format!(
                    "SwapChain: frame #{frame_idx} encoded bytes={} chunks={} changed={} copied={} tier={tier:?}",
                    stats.bytes_emitted, stats.chunks_submitted, stats.pixels_changed, stats.pixels_copied_hw
                ));
            }
            (stats.bytes_emitted, tier)
        }
        Err(st) => {
            crate::trace_log(&format!("SwapChain: encode/usb submit FAILED status=0x{:08X}", st as u32));
            drop(sink);
            if usb.recover_pipe() {
                let _ = usb.set_video_mode(mode);
                encoder.invalidate_shadow();
            }
            (0, QualityTier::Lossless)
        }
    }
}
