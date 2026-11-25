//! Standardized 1280x800 D3D11 Flip-Model benchmark workload for comparing
//! indirect display drivers (e.g., Rust dl_rust_idd vs Official DisplayLink)
//! directly on the Wacom DTU-1031 monitor.

#![allow(non_camel_case_types, non_snake_case, clippy::upper_case_acronyms)]

use core::ffi::c_void;
use core::mem::size_of;
use std::time::{Duration, Instant};

type HWND = *mut c_void;
type HINSTANCE = *mut c_void;
type HRESULT = i32;
type BOOL = i32;

const WS_POPUP: u32 = 0x8000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const PM_REMOVE: u32 = 0x0001;
const WM_QUIT: u32 = 0x0012;

const D3D_DRIVER_TYPE_HARDWARE: u32 = 1;
const D3D11_SDK_VERSION: u32 = 7;
const D3D11_CREATE_DEVICE_BGRA_SUPPORT: u32 = 0x20;
const DXGI_FORMAT_B8G8R8A8_UNORM: u32 = 87;
const DXGI_USAGE_RENDER_TARGET_OUTPUT: u32 = 0x20;
const DXGI_SWAP_EFFECT_FLIP_DISCARD: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy)]
struct GUID {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const IID_ID3D11TEXTURE2D: GUID = GUID {
    data1: 0x6f15_aaf2,
    data2: 0xd208,
    data3: 0x4e89,
    data4: [0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c],
};

#[repr(C)]
struct WNDCLASSW {
    style: u32,
    lpfn_wnd_proc: unsafe extern "system" fn(HWND, u32, usize, isize) -> isize,
    cb_cls_extra: i32,
    cb_wnd_extra: i32,
    h_instance: HINSTANCE,
    h_icon: *mut c_void,
    h_cursor: *mut c_void,
    h_br_background: *mut c_void,
    lpsz_menu_name: *const u16,
    lpsz_class_name: *const u16,
}

#[repr(C)]
#[derive(Default)]
struct MSG {
    hwnd: HWND,
    message: u32,
    w_param: usize,
    l_param: isize,
    time: u32,
    pt_x: i32,
    pt_y: i32,
}

#[repr(C)]
struct DXGI_RATIONAL {
    numerator: u32,
    denominator: u32,
}

#[repr(C)]
struct DXGI_MODE_DESC {
    width: u32,
    height: u32,
    refresh_rate: DXGI_RATIONAL,
    format: u32,
    scanline_ordering: u32,
    scaling: u32,
}

#[repr(C)]
struct DXGI_SAMPLE_DESC {
    count: u32,
    quality: u32,
}

#[repr(C)]
struct DXGI_SWAP_CHAIN_DESC {
    buffer_desc: DXGI_MODE_DESC,
    sample_desc: DXGI_SAMPLE_DESC,
    buffer_usage: u32,
    buffer_count: u32,
    output_window: HWND,
    windowed: BOOL,
    swap_effect: u32,
    flags: u32,
}
const _: () = assert!(size_of::<DXGI_SWAP_CHAIN_DESC>() == 72);

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct DXGI_FRAME_STATISTICS {
    present_count: u32,
    present_refresh_count: u32,
    sync_refresh_count: u32,
    _pad: u32,
    sync_qpc_time: i64,
    sync_gpu_time: i64,
}
const _: () = assert!(size_of::<DXGI_FRAME_STATISTICS>() == 32);

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(lp_wnd_class: *const WNDCLASSW) -> u16;
    fn CreateWindowExW(
        dw_ex_style: u32,
        lp_class_name: *const u16,
        lp_window_name: *const u16,
        dw_style: u32,
        x: i32,
        y: i32,
        n_width: i32,
        n_height: i32,
        h_wnd_parent: HWND,
        h_menu: *mut c_void,
        h_instance: HINSTANCE,
        lp_param: *mut c_void,
    ) -> HWND;
    fn DestroyWindow(hwnd: HWND) -> BOOL;
    fn DefWindowProcW(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize;
    fn PeekMessageW(lp_msg: *mut MSG, hwnd: HWND, min: u32, max: u32, remove: u32) -> BOOL;
    fn TranslateMessage(lp_msg: *const MSG) -> BOOL;
    fn DispatchMessageW(lp_msg: *const MSG) -> isize;
    fn SetProcessDPIAware() -> BOOL;
    fn SetProcessDpiAwarenessContext(value: isize) -> BOOL;
    fn ShowWindow(hwnd: HWND, n_cmd_show: i32) -> BOOL;
    fn SetWindowPos(
        hwnd: HWND,
        hwnd_insert_after: HWND,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        u_flags: u32,
    ) -> BOOL;
}

const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
const HWND_TOPMOST: HWND = -1isize as HWND;
const SW_SHOWNOACTIVATE: i32 = 4;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_SHOWWINDOW: u32 = 0x0040;

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
}

#[link(name = "d3d11")]
extern "system" {
    fn D3D11CreateDeviceAndSwapChain(
        p_adapter: *mut c_void,
        driver_type: u32,
        software: *mut c_void,
        flags: u32,
        p_feature_levels: *const u32,
        feature_levels: u32,
        sdk_version: u32,
        p_swap_chain_desc: *const DXGI_SWAP_CHAIN_DESC,
        pp_swap_chain: *mut *mut c_void,
        pp_device: *mut *mut c_void,
        p_feature_level: *mut u32,
        pp_immediate_context: *mut *mut c_void,
    ) -> HRESULT;
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, w: usize, l: isize) -> isize {
    DefWindowProcW(hwnd, msg, w, l)
}

unsafe fn com_release(ptr: *mut c_void) {
    if !ptr.is_null() {
        let vtbl = *(ptr as *const *const *const c_void);
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = core::mem::transmute(*vtbl.add(2));
        f(ptr);
    }
}

unsafe fn sc_present(sc: *mut c_void, sync_interval: u32) -> HRESULT {
    let vtbl = *(sc as *const *const *const c_void);
    let f: unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT =
        core::mem::transmute(*vtbl.add(8));
    f(sc, sync_interval, 0)
}

unsafe fn sc_get_buffer(sc: *mut c_void) -> *mut c_void {
    let vtbl = *(sc as *const *const *const c_void);
    let f: unsafe extern "system" fn(*mut c_void, u32, *const GUID, *mut *mut c_void) -> HRESULT =
        core::mem::transmute(*vtbl.add(9));
    let mut out = core::ptr::null_mut();
    let hr = f(sc, 0, &IID_ID3D11TEXTURE2D, &mut out);
    if hr >= 0 { out } else { core::ptr::null_mut() }
}

unsafe fn sc_get_frame_stats(sc: *mut c_void) -> Option<DXGI_FRAME_STATISTICS> {
    let vtbl = *(sc as *const *const *const c_void);
    let f: unsafe extern "system" fn(*mut c_void, *mut DXGI_FRAME_STATISTICS) -> HRESULT =
        core::mem::transmute(*vtbl.add(16));
    let mut st = DXGI_FRAME_STATISTICS::default();
    if f(sc, &mut st) >= 0 { Some(st) } else { None }
}

unsafe fn ctx_update_subresource(
    ctx: *mut c_void,
    dst: *mut c_void,
    data: *const u8,
    row_pitch: u32,
) {
    let vtbl = *(ctx as *const *const *const c_void);
    let f: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        u32,
        *const c_void,
        *const c_void,
        u32,
        u32,
    ) = core::mem::transmute(*vtbl.add(48));
    f(ctx, dst, 0, core::ptr::null(), data as *const c_void, row_pitch, 0);
}

#[derive(Clone, Copy)]
enum PhaseKind {
    Idle,
    UiScroll,
    VideoBox,
    FullMotion,
}

fn render_pattern(buf: &mut [u8], w: usize, h: usize, phase: PhaseKind, frame: usize) {
    let stride = w * 4;
    match phase {
        PhaseKind::Idle => {
            if frame == 0 {
                for y in 0..h {
                    for x in 0..w {
                        let o = y * stride + x * 4;
                        let (r, g, b) = if y < 48 {
                            (32, 36, 46)
                        } else if x < 200 {
                            (22, 25, 32)
                        } else {
                            (16, 18, 24)
                        };
                        buf[o] = b;
                        buf[o + 1] = g;
                        buf[o + 2] = r;
                        buf[o + 3] = 255;
                    }
                }
            }
        }
        PhaseKind::UiScroll => {
            let scroll = (frame * 8) % h;
            for y in 0..h {
                let vy = (y + scroll) % h;
                let band = vy % 64;
                let row_off = y * stride;
                for x in 0..w {
                    let o = row_off + x * 4;
                    let (r, g, b) = if x < 220 {
                        (24, 26, 34)
                    } else if (8..=54).contains(&band) && (240..=1220).contains(&x) {
                        if (16..=26).contains(&band)
                            && (260..=880).contains(&x)
                            && ((x / 6) % 3 != 0)
                        {
                            (230, 236, 245)
                        } else if (34..=42).contains(&band)
                            && (260..=700).contains(&x)
                            && ((x / 5) % 4 != 0)
                        {
                            (150, 165, 190)
                        } else {
                            (34, 38, 48)
                        }
                    } else {
                        (18, 20, 26)
                    };
                    buf[o] = b;
                    buf[o + 1] = g;
                    buf[o + 2] = r;
                    buf[o + 3] = 255;
                }
            }
        }
        PhaseKind::VideoBox => {
            if frame == 0 {
                render_pattern(buf, w, h, PhaseKind::Idle, 0);
            }
            let t = frame * 5;
            for y in 160..520 {
                let row_off = y * stride;
                for x in 320..960 {
                    let o = row_off + x * 4;
                    let r = ((x + t * 2) & 0xFF) as u8;
                    let g = ((y * 2 + t) & 0xFF) as u8;
                    let b = (((x ^ y).wrapping_add(t * 3)) & 0xFF) as u8;
                    buf[o] = b;
                    buf[o + 1] = g;
                    buf[o + 2] = r;
                    buf[o + 3] = 255;
                }
            }
        }
        PhaseKind::FullMotion => {
            let t = frame * 7;
            let box_x = (frame * 19) % (w - 220);
            let box_y = (frame * 13) % (h - 140);
            for y in 0..h {
                let row_off = y * stride;
                let y_wave = ((y * 3 + t) & 0xFF) as u8;
                for x in 0..w {
                    let o = row_off + x * 4;
                    if x >= box_x && x < box_x + 220 && y >= box_y && y < box_y + 140 {
                        buf[o] = 250;
                        buf[o + 1] = 220;
                        buf[o + 2] = 60;
                        buf[o + 3] = 255;
                    } else {
                        let r = ((x + t * 2) & 0xFF) as u8;
                        let g = y_wave.wrapping_add(((x >> 2) + t) as u8);
                        let b = (((x + y * 2) ^ t) & 0xFF) as u8;
                        buf[o] = b;
                        buf[o + 1] = g;
                        buf[o + 2] = r;
                        buf[o + 3] = 255;
                    }
                }
            }
        }
    }
}

fn pump_messages() -> bool {
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, core::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            if msg.message == WM_QUIT {
                return false;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    true
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("Usage: dl-bench <x> <y> <w> <h> [seconds_per_phase]");
        std::process::exit(2);
    }
    let mon_x: i32 = args[1].parse().unwrap();
    let mon_y: i32 = args[2].parse().unwrap();
    let w: usize = args[3].parse().unwrap();
    let h: usize = args[4].parse().unwrap();
    let phase_secs: u64 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(5);

    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
            SetProcessDPIAware();
        }
        let hinst = GetModuleHandleW(core::ptr::null());
        let cls: Vec<u16> = "DlRustBenchWnd\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            style: 0,
            lpfn_wnd_proc: wnd_proc,
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: hinst,
            h_icon: core::ptr::null_mut(),
            h_cursor: core::ptr::null_mut(),
            h_br_background: core::ptr::null_mut(),
            lpsz_menu_name: core::ptr::null(),
            lpsz_class_name: cls.as_ptr(),
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            cls.as_ptr(),
            cls.as_ptr(),
            WS_POPUP | WS_VISIBLE,
            mon_x,
            mon_y,
            w as i32,
            h as i32,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            hinst,
            core::ptr::null_mut(),
        );
        assert!(!hwnd.is_null(), "CreateWindowExW failed");
        // Explicitly enforce exact physical pixel coordinates on the target monitor.
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            mon_x,
            mon_y,
            w as i32,
            h as i32,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        let sc_desc = DXGI_SWAP_CHAIN_DESC {
            buffer_desc: DXGI_MODE_DESC {
                width: w as u32,
                height: h as u32,
                refresh_rate: DXGI_RATIONAL { numerator: 60, denominator: 1 },
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                scanline_ordering: 0,
                scaling: 0,
            },
            sample_desc: DXGI_SAMPLE_DESC { count: 1, quality: 0 },
            buffer_usage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            buffer_count: 2,
            output_window: hwnd,
            windowed: 1,
            swap_effect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            flags: 0,
        };

        let mut sc = core::ptr::null_mut();
        let mut dev = core::ptr::null_mut();
        let mut ctx = core::ptr::null_mut();
        let mut fl = 0u32;
        let hr = D3D11CreateDeviceAndSwapChain(
            core::ptr::null_mut(),
            D3D_DRIVER_TYPE_HARDWARE,
            core::ptr::null_mut(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            core::ptr::null(),
            0,
            D3D11_SDK_VERSION,
            &sc_desc,
            &mut sc,
            &mut dev,
            &mut fl,
            &mut ctx,
        );
        assert!(hr >= 0, "D3D11CreateDeviceAndSwapChain failed: 0x{:08X}", hr as u32);

        let mut frame_buf = vec![0u8; w * h * 4];
        let phases = [
            ("idle", PhaseKind::Idle, 3u64),
            ("ui_scroll", PhaseKind::UiScroll, phase_secs),
            ("video_box", PhaseKind::VideoBox, phase_secs),
            ("full_motion", PhaseKind::FullMotion, phase_secs),
        ];

        for (name, kind, secs) in phases {
            let start_stats = sc_get_frame_stats(sc);
            let phase_start = Instant::now();
            let target = Duration::from_secs(secs);
            let mut frame_times_ms: Vec<f64> = Vec::with_capacity((secs * 70) as usize);
            let mut frame_num = 0usize;

            // Warm-up 1 frame
            render_pattern(&mut frame_buf, w, h, kind, 0);
            let bb = sc_get_buffer(sc);
            if !bb.is_null() {
                ctx_update_subresource(ctx, bb, frame_buf.as_ptr(), (w * 4) as u32);
                com_release(bb);
            }
            let _ = sc_present(sc, 1);

            let mut last_tick = Instant::now();
            let frame_interval = Duration::from_nanos(16_666_667); // 60.0 Hz input cadence
            let mut next_frame_due = Instant::now() + frame_interval;
            while phase_start.elapsed() < target {
                if !pump_messages() {
                    break;
                }
                if matches!(kind, PhaseKind::Idle) {
                    std::thread::sleep(Duration::from_millis(16));
                    frame_num += 1;
                    continue;
                }
                render_pattern(&mut frame_buf, w, h, kind, frame_num);
                let bb = sc_get_buffer(sc);
                if !bb.is_null() {
                    ctx_update_subresource(ctx, bb, frame_buf.as_ptr(), (w * 4) as u32);
                    com_release(bb);
                }
                let _ = sc_present(sc, 0);
                // Pace cleanly to 60.0 Hz independent of primary 180 Hz monitor VSync.
                let now = Instant::now();
                if next_frame_due > now {
                    let rem = next_frame_due - now;
                    if rem > Duration::from_millis(2) {
                        std::thread::sleep(rem - Duration::from_millis(1));
                    }
                    while Instant::now() < next_frame_due {
                        core::hint::spin_loop();
                    }
                }
                let after = Instant::now();
                next_frame_due = (next_frame_due + frame_interval).max(after);
                let dt_ms = after.duration_since(last_tick).as_secs_f64() * 1000.0;
                last_tick = after;
                frame_times_ms.push(dt_ms);
                frame_num += 1;
            }

            let elapsed_s = phase_start.elapsed().as_secs_f64();
            let end_stats = sc_get_frame_stats(sc);
            let (dxgi_presents, dxgi_syncs) = match (start_stats, end_stats) {
                (Some(a), Some(b)) => (
                    b.present_count.saturating_sub(a.present_count),
                    b.sync_refresh_count.saturating_sub(a.sync_refresh_count),
                ),
                _ => (0, 0),
            };

            if frame_times_ms.is_empty() {
                println!(
                    "PHASE|{name}|secs={elapsed_s:.2}|frames={frame_num}|avg_fps=0.0|p50_ms=0.00|p95_ms=0.00|p99_ms=0.00|max_ms=0.00|stutter_gt20ms=0|dxgi_presents={dxgi_presents}|dxgi_syncs={dxgi_syncs}"
                );
            } else {
                frame_times_ms.sort_by(|a, b| a.total_cmp(b));
                let n = frame_times_ms.len();
                let avg_fps = n as f64 / elapsed_s;
                let p50 = frame_times_ms[n / 2];
                let p95 = frame_times_ms[(n * 95 / 100).min(n - 1)];
                let p99 = frame_times_ms[(n * 99 / 100).min(n - 1)];
                let max_ms = frame_times_ms[n - 1];
                let stutters = frame_times_ms.iter().filter(|&&t| t > 20.0).count();
                println!(
                    "PHASE|{name}|secs={elapsed_s:.2}|frames={n}|avg_fps={avg_fps:.1}|p50_ms={p50:.2}|p95_ms={p95:.2}|p99_ms={p99:.2}|max_ms={max_ms:.2}|stutter_gt20ms={stutters}|dxgi_presents={dxgi_presents}|dxgi_syncs={dxgi_syncs}"
                );
            }
        }

        com_release(ctx);
        com_release(dev);
        com_release(sc);
        DestroyWindow(hwnd);
    }
}
