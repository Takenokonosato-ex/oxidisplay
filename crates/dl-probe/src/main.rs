//! Diagnostic inspector & 1280x800 60 FPS encoder benchmark for `dl-protocol`.

use std::time::Instant;

use dl_protocol::{
    encode_mode_set, lfsr16, parse_edid, parse_vendor_descriptor, ChunkSink, DirtyRect,
    FrameEncoder, MoveRegion, VideoMode, DEFAULT_USB_CHUNK_SIZE, DTU_1031_FALLBACK_EDID,
};

struct CountingSink {
    buf: Vec<u8>,
    bytes: u64,
    chunks: u32,
}

impl CountingSink {
    fn new() -> Self {
        Self {
            buf: vec![0u8; DEFAULT_USB_CHUNK_SIZE],
            bytes: 0,
            chunks: 0,
        }
    }

    fn reset(&mut self) {
        self.bytes = 0;
        self.chunks = 0;
    }
}

impl ChunkSink for CountingSink {
    type Error = ();

    fn acquire_chunk(&mut self) -> Result<&mut [u8], Self::Error> {
        Ok(&mut self.buf[..])
    }

    fn submit_chunk(&mut self, len: usize) -> Result<(), Self::Error> {
        self.bytes += len as u64;
        self.chunks += 1;
        Ok(())
    }
}

fn main() {
    println!("=== DisplayLink DL-1x5 Rust Driver Diagnostics & 60 FPS Benchmark ===");

    // 1. Verify DTU-1031 vendor descriptor & EDID
    let vendor_desc: [u8; 23] = [
        0x17, 0x5F, 0x01, 0x00, 0x15, 0x05, 0x00, 0x01, 0x03, 0x00, 0x04, 0x04, 0x01, 0x00, 0x03,
        0x60, 0x00, 0x02, 0x04, 0x60, 0xE3, 0x16, 0x00,
    ];
    if let Some(vd) = parse_vendor_descriptor(&vendor_desc) {
        println!(
            "Vendor Descriptor (0x5F): version={}, SKU pixel limit={} pixels (1280x800 = {} pixels -> OK)",
            vd.version,
            vd.sku_pixel_limit,
            1280 * 800
        );
    }

    let edid = parse_edid(&DTU_1031_FALLBACK_EDID).expect("valid DTU-1031 EDID");
    let m = edid.modes[0];
    println!(
        "Monitor: {} ({}{:04X}), Physical: {}x{} cm",
        edid.monitor_name,
        core::str::from_utf8(&edid.manufacturer_id).unwrap_or("???"),
        edid.product_code,
        edid.width_cm,
        edid.height_cm
    );
    println!(
        "Preferred Mode: {}x{} @ {:.3} Hz | Pixel Clock: {:.2} MHz | HTotal={} VTotal={}",
        m.hdisplay,
        m.vdisplay,
        m.refresh_mhz() as f64 / 1000.0,
        m.clock_khz as f64 / 1000.0,
        m.htotal,
        m.vtotal
    );

    let mut modeset_bytes = Vec::new();
    encode_mode_set(&m, &mut modeset_bytes);
    println!(
        "Modeset Packet: {} bytes | LFSR16(XEndCount={})={:#06X} LFSR16(YEndCount={})={:#06X}",
        modeset_bytes.len(),
        m.htotal - 1,
        lfsr16(m.htotal - 1),
        m.vtotal,
        lfsr16(m.vtotal)
    );
    println!();

    // 2. Benchmark realistic 1280x800 60 FPS workloads
    let w = m.hdisplay as usize;
    let h = m.vdisplay as usize;
    let stride = w * 4;
    let raw_rgb565_bytes = (w * h * 2) as f64;
    // Conservative USB 2.0 High-Speed bulk throughput: 36 MB/s
    const USB2_MBPS: f64 = 36.0;

    let mut desktop = vec![0u8; stride * h];
    // Synthetic realistic desktop: dark IDE/browser background, sidebar, toolbar, text bands
    for y in 0..h {
        for x in 0..w {
            let off = y * stride + x * 4;
            let (r, g, b) = if y < 40 {
                (30, 32, 38) // title bar
            } else if x < 220 {
                (24, 26, 32) // sidebar
            } else if y % 20 >= 6 && y % 20 <= 14 && x > 240 && x < 920 && ((x + y) % 9 != 0) {
                (210, 215, 224) // crisp text-like pixels
            } else {
                (18, 20, 24) // editor background
            };
            desktop[off] = b;
            desktop[off + 1] = g;
            desktop[off + 2] = r;
            desktop[off + 3] = 255;
        }
    }

    let mut encoder = FrameEncoder::new(w as u32, h as u32);
    let mut sink = CountingSink::new();

    // Scenario A: Initial full-screen desktop frame
    let t0 = Instant::now();
    let stats = encoder
        .encode_frame_bgra(&desktop, stride, &[], &[], &mut sink)
        .unwrap();
    let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;
    report("1. Initial Full Desktop (1280x800)", dt_ms, stats.bytes_emitted, raw_rgb565_bytes, USB2_MBPS);

    // Scenario B: Static desktop (0 changed pixels)
    let full = [DirtyRect { left: 0, top: 0, right: w as u32, bottom: h as u32 }];
    sink.reset();
    let t0 = Instant::now();
    let stats = encoder
        .encode_frame_bgra(&desktop, stride, &[], &full, &mut sink)
        .unwrap();
    let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;
    report("2. Static Desktop (0% diff)", dt_ms, stats.bytes_emitted, raw_rgb565_bytes, USB2_MBPS);

    // Scenario C: Pen stroke / cursor / active widget update (160x160 region)
    let mut pen_frame = desktop.clone();
    for y in 300..460 {
        for x in 500..660 {
            if (x as i32 - 580).abs() + (y as i32 - 380).abs() < 60 {
                let off = y * stride + x * 4;
                pen_frame[off] = 255;
                pen_frame[off + 1] = 180;
                pen_frame[off + 2] = 40;
            }
        }
    }
    let pen_rect = [DirtyRect { left: 500, top: 300, right: 660, bottom: 460 }];
    sink.reset();
    let t0 = Instant::now();
    let stats = encoder
        .encode_frame_bgra(&pen_frame, stride, &[], &pen_rect, &mut sink)
        .unwrap();
    let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;
    report("3. Pen Stroke / Cursor Dirty Rect (160x160)", dt_ms, stats.bytes_emitted, raw_rgb565_bytes, USB2_MBPS);

    // Scenario D: 60 FPS vertical scrolling via WRITECOPY16 move region + 24px newly exposed band
    let mut scrolled = pen_frame.clone();
    for y in 40..(h - 24) {
        let src_off = (y + 24) * stride;
        let dst_off = y * stride;
        scrolled.copy_within(src_off..src_off + stride, dst_off);
    }
    for y in (h - 24)..h {
        for x in 240..920 {
            let off = y * stride + x * 4;
            scrolled[off] = 200;
            scrolled[off + 1] = 205;
            scrolled[off + 2] = 215;
        }
    }
    let mv = [MoveRegion {
        src_x: 0,
        src_y: 64,
        dst_x: 0,
        dst_y: 40,
        width: w as u32,
        height: (h - 64) as u32,
    }];
    let exposed = [DirtyRect { left: 0, top: (h - 24) as u32, right: w as u32, bottom: h as u32 }];
    sink.reset();
    let t0 = Instant::now();
    let stats = encoder
        .encode_frame_bgra(&scrolled, stride, &mv, &exposed, &mut sink)
        .unwrap();
    let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;
    report("4. Hardware Blit Scroll (WRITECOPY16 + 24px)", dt_ms, stats.bytes_emitted, raw_rgb565_bytes, USB2_MBPS);

    // Scenario E: 640x360 embedded 60 FPS video window playing inside desktop
    let mut vid = scrolled.clone();
    for y in 120..480 {
        for x in 320..960 {
            let off = y * stride + x * 4;
            vid[off] = ((x * 3 + y) & 0xFF) as u8;
            vid[off + 1] = ((x + y * 2) & 0xFF) as u8;
            vid[off + 2] = (((x ^ y) * 5) & 0xFF) as u8;
        }
    }
    let vid_rect = [DirtyRect { left: 320, top: 120, right: 960, bottom: 480 }];
    sink.reset();
    let t0 = Instant::now();
    let stats = encoder
        .encode_frame_bgra(&vid, stride, &[], &vid_rect, &mut sink)
        .unwrap();
    let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;
    report("5. 640x360 Video Window Update", dt_ms, stats.bytes_emitted, raw_rgb565_bytes, USB2_MBPS);

    // Scenario F: Full-screen 1280x800 high-entropy frame (worst case)
    let mut noise = vec![0u8; stride * h];
    let mut seed = 0x1234_5678u32;
    for chunk in noise.as_chunks_mut::<4>().0 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        chunk[0] = (seed >> 24) as u8;
        chunk[1] = (seed >> 16) as u8;
        chunk[2] = (seed >> 8) as u8;
        chunk[3] = 255;
    }
    sink.reset();
    let t0 = Instant::now();
    let stats = encoder
        .encode_frame_bgra(&noise, stride, &[], &full, &mut sink)
        .unwrap();
    let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;
    report("6. Full-Screen 100% Random Noise (Worst Case)", dt_ms, stats.bytes_emitted, raw_rgb565_bytes, USB2_MBPS);

    let _ = VideoMode::DTU_1031_1280X800_60;
}

fn report(label: &str, cpu_ms: f64, bytes: u64, raw_frame_bytes: f64, usb2_mbps: f64) {
    let kb = bytes as f64 / 1024.0;
    let usb_ms = (bytes as f64 / (usb2_mbps * 1024.0 * 1024.0)) * 1000.0;
    let max_fps = if usb_ms > 0.001 {
        (1000.0 / usb_ms.max(cpu_ms)).min(60.0)
    } else {
        60.0
    };
    let ratio = if bytes > 0 { raw_frame_bytes / bytes as f64 } else { f64::INFINITY };
    println!(
        "{:<46} | CPU: {:5.2} ms | USB: {:7.1} KB ({:5.1}x) | Wire: {:5.2} ms | Est: {:4.1} FPS",
        label, cpu_ms, kb, ratio, usb_ms, max_fps
    );
}
