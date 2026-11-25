//! Zero-allocation DisplayLink DL-1x5 frame encoder with adaptive 60 FPS motion scaling
//! and automatic lossless static refinement.
//!
//! - Low/moderate traffic: 100% lossless RGB565 with multi-span shadow-buffer diffing.
//! - High motion (USB 2.0 saturation): smoothly steps into `MotionFast` (temporal micro-noise
//!   deadzone + RGB454 RLE-boost) or `MotionUltra` (RGB444 RLE-boost + alternating even/odd
//!   scanlines) to keep motion locked near 60 FPS over USB 2.0.
//! - The moment motion pauses, `refine_lossless` restores all degraded rows to bit-exact RGB565.

use crate::huffman::{encode_comp16_command, encode_comp8_command};
use crate::regs::{
    UDL_CMD_WRITECOPY16, UDL_CMD_WRITECOPY8, UDL_CMD_WRITERLX16, UDL_CMD_WRITERLX8, UDL_MSG_BULK,
};

/// USB 2.0 High-Speed bulk transfer chunk size (128 KiB - 512 B = 130,560 bytes, multiple of 512 B).
/// Larger chunks halve per-IRP WDF/WinUSB completion overhead for higher sustained throughput.
pub const DEFAULT_USB_CHUNK_SIZE: usize = 128 * 1024 - 512;

/// Minimum bytes needed in a chunk to emit a `WRITERLX16` command (7-byte header + 4-byte payload).
const MIN_RLX_CMD_BYTES: usize = 11;
/// Exact bytes needed for a `WRITECOPY16` / `WRITECOPY8` command (2-byte prefix + 3 dst + 1 count + 3 src).
const COPY_CMD_BYTES: usize = 9;
/// Minimum unchanged pixel run length required before splitting a dirty scanline span.
const MIN_UNCHANGED_GAP: usize = 6;
/// Maximum pixels per DisplayLink command (0 on wire encodes 256).
const MAX_CMD_PIXELS: usize = 256;

/// Adaptive quality tier chosen dynamically based on live USB 2.0 backpressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityTier {
    /// 100% exact RGB565 (sharpest text and colors).
    Lossless,
    /// Motion RLE-boost: RGB454 quantization + 1-step temporal deadzone (~2-3x smaller USB traffic).
    MotionFast,
    /// Extreme full-screen motion: RGB444 RLE-boost + alternating even/odd scanlines (~4-6x smaller).
    MotionUltra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyRect {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl DirtyRect {
    #[inline]
    pub fn clamp(self, width: u32, height: u32) -> Option<Self> {
        let left = self.left.min(width);
        let right = self.right.min(width);
        let top = self.top.min(height);
        let bottom = self.bottom.min(height);
        if left < right && top < bottom {
            Some(Self { left, top, right, bottom })
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveRegion {
    pub src_x: u32,
    pub src_y: u32,
    pub dst_x: u32,
    pub dst_y: u32,
    pub width: u32,
    pub height: u32,
}

/// Statistics for a single encoded frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FrameStats {
    pub pixels_examined: u64,
    pub pixels_changed: u64,
    pub pixels_copied_hw: u64,
    pub bytes_emitted: u64,
    pub chunks_submitted: u32,
}

/// Sink that provides fixed-size writable USB chunk buffers and accepts filled chunks.
pub trait ChunkSink {
    type Error;

    fn acquire_chunk(&mut self) -> Result<&mut [u8], Self::Error>;
    fn submit_chunk(&mut self, len: usize) -> Result<(), Self::Error>;
}

/// Simple `Vec<Vec<u8>>` sink for unit tests and offline benchmarks.
#[derive(Debug, Default)]
pub struct VecChunkSink {
    pub chunk_size: usize,
    pub current: Vec<u8>,
    pub submitted: Vec<Vec<u8>>,
}

impl VecChunkSink {
    pub fn new(chunk_size: usize) -> Self {
        Self {
            chunk_size,
            current: vec![0u8; chunk_size],
            submitted: Vec::new(),
        }
    }
}

impl ChunkSink for VecChunkSink {
    type Error = ();

    fn acquire_chunk(&mut self) -> Result<&mut [u8], Self::Error> {
        if self.current.len() != self.chunk_size {
            self.current.resize(self.chunk_size, 0);
        }
        Ok(&mut self.current[..])
    }

    fn submit_chunk(&mut self, len: usize) -> Result<(), Self::Error> {
        self.submitted.push(self.current[..len].to_vec());
        Ok(())
    }
}

/// Convert a BGRA8888 pixel (`[B, G, R, A]`) to native-endian RGB565 `u16` (`RRRRR_GGGGGG_BBBBB`).
#[inline(always)]
pub const fn bgra_to_rgb565(b: u8, g: u8, r: u8) -> u16 {
    (((r as u16) & 0xF8) << 8) | (((g as u16) & 0xFC) << 3) | ((b as u16) >> 3)
}

/// Pack the lower 3-2-3 bits of a BGRA8888 pixel into DL-1x5 `base8` `RGB323` format:
/// `[R2 R1 R0 | G1 G0 | B2 B1 B0]` (verified against `libdlo` `DLO_RGB8`).
#[inline(always)]
pub const fn bgra_to_rgb323_low(b: u8, g: u8, r: u8) -> u8 {
    ((r & 0x07) << 5) | ((g & 0x03) << 3) | (b & 0x07)
}

/// Fast row-wide BGRA8888 -> RGB565 conversion with runtime AVX2 / SSE2 acceleration on x86_64.
/// Optional `quant_mask` (`0xFFFF` = lossless, `0xF7DE` = RGB454, `0xF79E` = RGB444) is folded
/// directly into the SIMD register pass at zero extra cost.
#[inline]
fn convert_bgra_row_rgb565(src: &[u8], dst: &mut [u16], quant_mask: u16) {
    debug_assert_eq!(src.len(), dst.len() * 4);
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            unsafe { convert_bgra_row_avx2(src, dst, quant_mask) };
            return;
        }
        unsafe { convert_bgra_row_sse2(src, dst, quant_mask) };
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        for (o, px) in dst.iter_mut().zip(src.chunks_exact(4)) {
            *o = bgra_to_rgb565(px[0], px[1], px[2]) & quant_mask;
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn convert_bgra_row_avx2(src: &[u8], dst: &mut [u16], quant_mask: u16) {
    use core::arch::x86_64::*;
    let len = dst.len();
    let mut i = 0usize;
    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    let mask_r = _mm256_set1_epi32(0x00F8_0000u32 as i32);
    let mask_g = _mm256_set1_epi32(0x0000_FC00u32 as i32);
    let mask_b = _mm256_set1_epi32(0x0000_00F8u32 as i32);
    let qmask = _mm256_set1_epi16(quant_mask as i16);

    // Process 16 pixels (64 bytes BGRA -> 32 bytes RGB565) per iteration.
    while i + 16 <= len {
        let p0 = _mm256_loadu_si256(src_ptr.add(i * 4) as *const __m256i);
        let p1 = _mm256_loadu_si256(src_ptr.add((i + 8) * 4) as *const __m256i);

        let r0 = _mm256_srli_epi32(_mm256_and_si256(p0, mask_r), 8);
        let g0 = _mm256_srli_epi32(_mm256_and_si256(p0, mask_g), 5);
        let b0 = _mm256_srli_epi32(_mm256_and_si256(p0, mask_b), 3);
        let rgb0 = _mm256_or_si256(_mm256_or_si256(r0, g0), b0);

        let r1 = _mm256_srli_epi32(_mm256_and_si256(p1, mask_r), 8);
        let g1 = _mm256_srli_epi32(_mm256_and_si256(p1, mask_g), 5);
        let b1 = _mm256_srli_epi32(_mm256_and_si256(p1, mask_b), 3);
        let rgb1 = _mm256_or_si256(_mm256_or_si256(r1, g1), b1);

        // Pack 32-bit lanes to 16-bit unsigned values and fix AVX2 128-bit lane order.
        let packed = _mm256_packus_epi32(rgb0, rgb1);
        let ordered = _mm256_permute4x64_epi64(packed, 0xD8);
        let masked = _mm256_and_si256(ordered, qmask);
        _mm256_storeu_si256(dst_ptr.add(i) as *mut __m256i, masked);
        i += 16;
    }

    while i < len {
        let o = i * 4;
        *dst_ptr.add(i) = bgra_to_rgb565(*src_ptr.add(o), *src_ptr.add(o + 1), *src_ptr.add(o + 2)) & quant_mask;
        i += 1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn convert_bgra_row_sse2(src: &[u8], dst: &mut [u16], quant_mask: u16) {
    use core::arch::x86_64::*;
    let len = dst.len();
    let mut i = 0usize;
    let src_ptr = src.as_ptr();
    let dst_ptr = dst.as_mut_ptr();

    let mask_r = _mm_set1_epi32(0x00F8_0000u32 as i32);
    let mask_g = _mm_set1_epi32(0x0000_FC00u32 as i32);
    let mask_b = _mm_set1_epi32(0x0000_00F8u32 as i32);
    let qmask = _mm_set1_epi16(quant_mask as i16);
    let bias = _mm_set1_epi32(0x8000);
    let unbias = _mm_set1_epi16(0x8000u16 as i16);

    while i + 8 <= len {
        let p0 = _mm_loadu_si128(src_ptr.add(i * 4) as *const __m128i);
        let p1 = _mm_loadu_si128(src_ptr.add((i + 4) * 4) as *const __m128i);

        let rgb0 = _mm_or_si128(
            _mm_or_si128(
                _mm_srli_epi32(_mm_and_si128(p0, mask_r), 8),
                _mm_srli_epi32(_mm_and_si128(p0, mask_g), 5),
            ),
            _mm_srli_epi32(_mm_and_si128(p0, mask_b), 3),
        );
        let rgb1 = _mm_or_si128(
            _mm_or_si128(
                _mm_srli_epi32(_mm_and_si128(p1, mask_r), 8),
                _mm_srli_epi32(_mm_and_si128(p1, mask_g), 5),
            ),
            _mm_srli_epi32(_mm_and_si128(p1, mask_b), 3),
        );

        // SSE2 has signed packss_epi32; bias by -32768 before packing then XOR 0x8000 back.
        let packed = _mm_xor_si128(
            _mm_packs_epi32(_mm_sub_epi32(rgb0, bias), _mm_sub_epi32(rgb1, bias)),
            unbias,
        );
        _mm_storeu_si128(dst_ptr.add(i) as *mut __m128i, _mm_and_si128(packed, qmask));
        i += 8;
    }

    while i < len {
        let o = i * 4;
        *dst_ptr.add(i) = bgra_to_rgb565(*src_ptr.add(o), *src_ptr.add(o + 1), *src_ptr.add(o + 2)) & quant_mask;
        i += 1;
    }
}

/// Fast SIMD scan to find the first differing index in `a[start..]` vs `b[start..]`.
#[inline]
fn find_first_diff_u16(a: &[u16], b: &[u16], mut idx: usize) -> usize {
    let len = a.len().min(b.len());
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            unsafe {
                use core::arch::x86_64::*;
                while idx + 16 <= len {
                    let va = _mm256_loadu_si256(a.as_ptr().add(idx) as *const __m256i);
                    let vb = _mm256_loadu_si256(b.as_ptr().add(idx) as *const __m256i);
                    let eq = _mm256_cmpeq_epi16(va, vb);
                    let mask = _mm256_movemask_epi8(eq) as u32;
                    if mask != 0xFFFF_FFFF {
                        let first_diff_byte = (!mask).trailing_zeros() as usize;
                        return idx + (first_diff_byte / 2);
                    }
                    idx += 16;
                }
            }
        }
    }
    while idx < len && a[idx] == b[idx] {
        idx += 1;
    }
    idx
}

/// 4x4 ordered Bayer dithering matrix.
const BAYER_4X4: [[u8; 4]; 4] = [
    [0, 8, 2, 10],
    [12, 4, 14, 6],
    [3, 11, 1, 9],
    [15, 7, 13, 5],
];

/// Convert BGRA8888 to RGB565 with subtle 4x4 Bayer ordered dithering.
#[inline(always)]
pub const fn bgra_to_rgb565_dither(b: u8, g: u8, r: u8, x: usize, y: usize) -> u16 {
    let t = BAYER_4X4[y & 3][x & 3];
    let r_d = r.saturating_add(t >> 1);
    let g_d = g.saturating_add(t >> 2);
    let b_d = b.saturating_add(t >> 1);
    bgra_to_rgb565(b_d, g_d, r_d)
}

/// Returns true if two RGB565 pixels are within `tol` steps on every channel (temporal deadzone).
#[inline(always)]
const fn rgb565_within_tolerance(a: u16, b: u16, tol: i32) -> bool {
    let dr = ((a >> 11) as i32) - ((b >> 11) as i32);
    let dg = (((a >> 5) & 0x3F) as i32) - (((b >> 5) & 0x3F) as i32);
    let db = ((a & 0x1F) as i32) - ((b & 0x1F) as i32);
    dr >= -tol && dr <= tol && dg >= -(tol * 2) && dg <= (tol * 2) && db >= -tol && db <= tol
}

/// Pre-allocated DisplayLink DL-1x5 frame encoder with shadow framebuffer diffing,
/// adaptive motion quantization, optional hardware Huffman (`0xAF 0x78`), and
/// automatic lossless static refinement (including optional 24-bit `base8` plane).
pub struct FrameEncoder {
    width: u32,
    height: u32,
    shadow_fb: Vec<u16>,
    row_scratch: Vec<u16>,
    /// Tracks which scanlines were quantized or skipped during motion and need lossless RGB565 refinement.
    row_degraded: Vec<bool>,
    degraded_row_count: usize,
    /// Optional 24-bit lower-bits (`base8` `RGB323`) shadow plane & dirty-row tracking.
    shadow_fb8: Vec<u8>,
    row_scratch8: Vec<u8>,
    row_base8_dirty: Vec<bool>,
    base8_dirty_count: usize,
    color_depth_24: bool,
    /// Enable adaptive per-span hardware Huffman (`0xAF 0x78` / `0x70`) after uploading `0xAF 0xE0`.
    huffman_enabled: bool,
    interlace_parity: usize,
    force_full_refresh: bool,
    dither: bool,
}

impl FrameEncoder {
    pub fn new(width: u32, height: u32) -> Self {
        let count = (width as usize).saturating_mul(height as usize);
        Self {
            width,
            height,
            shadow_fb: vec![0u16; count],
            row_scratch: vec![0u16; width as usize],
            row_degraded: vec![false; height as usize],
            degraded_row_count: 0,
            shadow_fb8: Vec::new(),
            row_scratch8: Vec::new(),
            row_base8_dirty: Vec::new(),
            base8_dirty_count: 0,
            color_depth_24: false,
            huffman_enabled: false,
            interlace_parity: 0,
            force_full_refresh: true,
            dither: false,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }
        self.width = width;
        self.height = height;
        let count = (width as usize).saturating_mul(height as usize);
        self.shadow_fb.clear();
        self.shadow_fb.resize(count, 0);
        self.row_scratch.clear();
        self.row_scratch.resize(width as usize, 0);
        self.row_degraded.clear();
        self.row_degraded.resize(height as usize, false);
        self.degraded_row_count = 0;
        if self.color_depth_24 {
            self.shadow_fb8.clear();
            self.shadow_fb8.resize(count, 0);
            self.row_scratch8.clear();
            self.row_scratch8.resize(width as usize, 0);
            self.row_base8_dirty.clear();
            self.row_base8_dirty.resize(height as usize, true);
            self.base8_dirty_count = height as usize;
        }
        self.force_full_refresh = true;
    }

    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    #[inline]
    pub fn set_dither(&mut self, enabled: bool) {
        self.dither = enabled;
    }

    /// Enable or disable adaptive per-span hardware Huffman (`0xAF 0x78` / `0x70`).
    /// Call only after sending `encode_load_huffman_table` (`0xAF 0xE0`) to the device.
    #[inline]
    pub fn set_huffman_enabled(&mut self, enabled: bool) {
        self.huffman_enabled = enabled;
    }

    #[inline]
    pub fn huffman_enabled(&self) -> bool {
        self.huffman_enabled
    }

    /// Enable or disable static 24-bit color refinement via the DL-1x5 `base8` (`RGB323`) plane.
    /// Pre-allocates the 1-byte/pixel `shadow_fb8` buffer once so per-frame encoding stays zero-alloc.
    pub fn set_color_depth_24(&mut self, enabled: bool) {
        if self.color_depth_24 == enabled {
            return;
        }
        self.color_depth_24 = enabled;
        if enabled {
            let count = (self.width as usize).saturating_mul(self.height as usize);
            self.shadow_fb8.clear();
            self.shadow_fb8.resize(count, 0);
            self.row_scratch8.clear();
            self.row_scratch8.resize(self.width as usize, 0);
            self.row_base8_dirty.clear();
            self.row_base8_dirty.resize(self.height as usize, true);
            self.base8_dirty_count = self.height as usize;
        } else {
            self.base8_dirty_count = 0;
        }
    }

    #[inline]
    pub fn color_depth_24(&self) -> bool {
        self.color_depth_24
    }

    #[inline]
    pub fn invalidate_shadow(&mut self) {
        self.force_full_refresh = true;
        if self.color_depth_24 {
            self.shadow_fb8.fill(0);
            self.row_base8_dirty.fill(true);
            self.base8_dirty_count = self.height as usize;
        }
    }

    /// Returns true if any scanlines await lossless RGB565 or static 24-bit `base8` refinement.
    #[inline]
    pub fn needs_refinement(&self) -> bool {
        self.degraded_row_count > 0 || (self.color_depth_24 && self.base8_dirty_count > 0)
    }

    #[inline]
    pub fn shadow_fb(&self) -> &[u16] {
        &self.shadow_fb
    }

    #[inline]
    pub fn shadow_fb8(&self) -> &[u8] {
        &self.shadow_fb8
    }

    /// Encode a frame at 100% lossless RGB565 (convenience wrapper).
    pub fn encode_frame_bgra<S: ChunkSink>(
        &mut self,
        surface: &[u8],
        stride_bytes: usize,
        move_regions: &[MoveRegion],
        dirty_rects: &[DirtyRect],
        sink: &mut S,
    ) -> Result<FrameStats, S::Error> {
        self.encode_frame_bgra_adaptive(
            surface,
            stride_bytes,
            move_regions,
            dirty_rects,
            QualityTier::Lossless,
            sink,
        )
    }

    /// Encode a frame with the requested adaptive `QualityTier`.
    ///
    /// When `dirty_rects` and `move_regions` are both empty (idle / static re-encode frame from DWM)
    /// and `needs_refinement()` is true, restores degraded scanlines to bit-exact lossless RGB565.
    pub fn encode_frame_bgra_adaptive<S: ChunkSink>(
        &mut self,
        surface: &[u8],
        stride_bytes: usize,
        move_regions: &[MoveRegion],
        dirty_rects: &[DirtyRect],
        tier: QualityTier,
        sink: &mut S,
    ) -> Result<FrameStats, S::Error> {
        let mut stats = FrameStats::default();
        let w = self.width;
        let h = self.height;
        if w == 0 || h == 0 {
            return Ok(stats);
        }

        let mut writer = ChunkWriter::new(sink, self.huffman_enabled);

        // Idle / static refinement pass:
        // 1. Restore degraded RGB565 rows to 100% lossless first.
        // 2. Once RGB565 is lossless, refine dirty 24-bit `base8` rows (`RGB323` lower bits).
        if !self.force_full_refresh && move_regions.is_empty() && dirty_rects.is_empty() {
            if self.degraded_row_count > 0 {
                self.refine_degraded_rows(surface, stride_bytes, 420, &mut writer, &mut stats)?;
            } else if self.color_depth_24 && self.base8_dirty_count > 0 {
                self.refine_base8_rows(surface, stride_bytes, 420, &mut writer, &mut stats)?;
            }
            writer.finish(&mut stats)?;
            return Ok(stats);
        }

        // 1. Hardware screen-to-screen blits (`WRITECOPY16` + optional `WRITECOPY8`) for move regions.
        if !self.force_full_refresh && !move_regions.is_empty() {
            for mv in move_regions {
                self.encode_move_region(*mv, &mut writer, &mut stats)?;
            }
        }

        // 2. Dirty rectangles (or full screen on first frame / invalidation).
        let full_screen = [DirtyRect {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        }];
        let rects: &[DirtyRect] = if self.force_full_refresh {
            &full_screen
        } else {
            dirty_rects
        };

        let force = self.force_full_refresh;
        let effective_tier = if force { QualityTier::Lossless } else { tier };
        let parity = self.interlace_parity;
        if effective_tier == QualityTier::MotionUltra {
            self.interlace_parity ^= 1;
        }

        for &rect in rects {
            let Some(rect) = rect.clamp(w, h) else {
                continue;
            };
            self.encode_dirty_rect_bgra(
                surface,
                stride_bytes,
                rect,
                force,
                effective_tier,
                parity,
                &mut writer,
                &mut stats,
            )?;
        }

        // Opportunistic static refinement when back in Lossless mode with spare USB headroom.
        if effective_tier == QualityTier::Lossless && !force {
            if self.degraded_row_count > 0 {
                self.refine_degraded_rows(surface, stride_bytes, 200, &mut writer, &mut stats)?;
            } else if self.color_depth_24
                && self.base8_dirty_count > 0
                && stats.pixels_changed < 80_000
            {
                self.refine_base8_rows(surface, stride_bytes, 120, &mut writer, &mut stats)?;
            }
        }

        self.force_full_refresh = false;
        writer.finish(&mut stats)?;
        Ok(stats)
    }

    #[inline]
    fn mark_row_degraded(&mut self, y: usize) {
        if !self.row_degraded[y] {
            self.row_degraded[y] = true;
            self.degraded_row_count += 1;
        }
    }

    #[inline]
    fn clear_row_degraded(&mut self, y: usize) {
        if self.row_degraded[y] {
            self.row_degraded[y] = false;
            self.degraded_row_count = self.degraded_row_count.saturating_sub(1);
        }
    }

    #[inline]
    fn mark_row_base8_dirty(&mut self, y: usize) {
        if self.color_depth_24 && !self.row_base8_dirty[y] {
            self.row_base8_dirty[y] = true;
            self.base8_dirty_count += 1;
        }
    }

    #[inline]
    fn clear_row_base8_dirty(&mut self, y: usize) {
        if self.color_depth_24 && self.row_base8_dirty[y] {
            self.row_base8_dirty[y] = false;
            self.base8_dirty_count = self.base8_dirty_count.saturating_sub(1);
        }
    }

    /// Upgrade up to `max_rows` dirty scanlines on the 8-bit `base8` (`RGB323`) plane
    /// for full 24-bit color when motion settles (zero bandwidth penalty during motion!).
    fn refine_base8_rows<S: ChunkSink>(
        &mut self,
        surface: &[u8],
        stride_bytes: usize,
        max_rows: usize,
        writer: &mut ChunkWriter<'_, S>,
        stats: &mut FrameStats,
    ) -> Result<(), S::Error> {
        let fb_w = self.width as usize;
        let fb_h = self.height as usize;
        let base8_addr = (fb_w * fb_h * 2) as u32;
        let mut refined = 0usize;

        for y in 0..fb_h {
            if refined >= max_rows || self.base8_dirty_count == 0 {
                break;
            }
            if !self.row_base8_dirty[y] {
                continue;
            }

            let src_off = y.saturating_mul(stride_bytes);
            let src_end = src_off + fb_w * 4;
            if src_end > surface.len() {
                break;
            }
            let src_bytes = &surface[src_off..src_end];
            let scratch8 = &mut self.row_scratch8[..fb_w];
            for (dst, px) in scratch8.iter_mut().zip(src_bytes.as_chunks::<4>().0) {
                *dst = bgra_to_rgb323_low(px[0], px[1], px[2]);
            }

            let shadow_row = &mut self.shadow_fb8[y * fb_w..(y + 1) * fb_w];
            let mut idx = 0usize;
            while idx < fb_w {
                while idx < fb_w && scratch8[idx] == shadow_row[idx] {
                    idx += 1;
                }
                if idx >= fb_w {
                    break;
                }
                let span_start = idx;
                let mut last_diff = idx;
                idx += 1;
                let mut same_run = 0usize;
                while idx < fb_w {
                    if scratch8[idx] == shadow_row[idx] {
                        same_run += 1;
                        if same_run >= MIN_UNCHANGED_GAP {
                            break;
                        }
                    } else {
                        same_run = 0;
                        last_diff = idx;
                    }
                    idx += 1;
                }
                let span_end = last_diff + 1;
                shadow_row[span_start..span_end].copy_from_slice(&scratch8[span_start..span_end]);
                let dev_addr = base8_addr + (y * fb_w + span_start) as u32;
                writer.emit_span8(dev_addr, &scratch8[span_start..span_end], stats)?;
            }

            self.clear_row_base8_dirty(y);
            refined += 1;
        }
        Ok(())
    }

    /// Restore up to `max_rows` degraded scanlines back to 100% bit-exact lossless RGB565.
    fn refine_degraded_rows<S: ChunkSink>(
        &mut self,
        surface: &[u8],
        stride_bytes: usize,
        max_rows: usize,
        writer: &mut ChunkWriter<'_, S>,
        stats: &mut FrameStats,
    ) -> Result<(), S::Error> {
        let w = self.width;
        let h = self.height as usize;
        let mut refined = 0usize;

        for y in 0..h {
            if refined >= max_rows || self.degraded_row_count == 0 {
                break;
            }
            if !self.row_degraded[y] {
                continue;
            }
            let rect = DirtyRect {
                left: 0,
                top: y as u32,
                right: w,
                bottom: (y + 1) as u32,
            };
            self.encode_dirty_rect_bgra(
                surface,
                stride_bytes,
                rect,
                false,
                QualityTier::Lossless,
                0,
                writer,
                stats,
            )?;
            self.clear_row_degraded(y);
            refined += 1;
        }
        Ok(())
    }

    fn encode_move_region<S: ChunkSink>(
        &mut self,
        mv: MoveRegion,
        writer: &mut ChunkWriter<'_, S>,
        stats: &mut FrameStats,
    ) -> Result<(), S::Error> {
        let w = self.width as usize;
        let h = self.height as usize;
        if mv.width == 0 || mv.height == 0 {
            return Ok(());
        }
        if mv.src_x.saturating_add(mv.width) as usize > w
            || mv.dst_x.saturating_add(mv.width) as usize > w
            || mv.src_y.saturating_add(mv.height) as usize > h
            || mv.dst_y.saturating_add(mv.height) as usize > h
        {
            return Ok(());
        }
        if mv.src_x == mv.dst_x && mv.src_y == mv.dst_y {
            return Ok(());
        }

        let mw = mv.width as usize;
        let mh = mv.height as usize;
        let reverse_y = mv.dst_y > mv.src_y;
        let reverse_x = mv.dst_y == mv.src_y && mv.dst_x > mv.src_x;

        for row_idx in 0..mh {
            let dy_off = if reverse_y { mh - 1 - row_idx } else { row_idx };
            let sy = mv.src_y as usize + dy_off;
            let dy = mv.dst_y as usize + dy_off;
            let src_row_base = sy * w + mv.src_x as usize;
            let dst_row_base = dy * w + mv.dst_x as usize;

            self.shadow_fb
                .copy_within(src_row_base..src_row_base + mw, dst_row_base);
            if self.row_degraded[sy] {
                self.mark_row_degraded(dy);
            }
            if self.color_depth_24 {
                self.shadow_fb8
                    .copy_within(src_row_base..src_row_base + mw, dst_row_base);
                if self.row_base8_dirty[sy] {
                    self.mark_row_base8_dirty(dy);
                }
            }

            let base8 = (w * h * 2) as u32;
            let mut remaining = mw;
            if reverse_x {
                let mut offset = mw;
                while remaining > 0 {
                    let count = remaining.min(MAX_CMD_PIXELS);
                    offset -= count;
                    let src_addr = ((src_row_base + offset) * 2) as u32;
                    let dst_addr = ((dst_row_base + offset) * 2) as u32;
                    writer.emit_copy16(dst_addr, src_addr, count, stats)?;
                    if self.color_depth_24 {
                        writer.emit_copy8(
                            base8 + (dst_row_base + offset) as u32,
                            base8 + (src_row_base + offset) as u32,
                            count,
                            stats,
                        )?;
                    }
                    remaining -= count;
                }
            } else {
                let mut offset = 0usize;
                while remaining > 0 {
                    let count = remaining.min(MAX_CMD_PIXELS);
                    let src_addr = ((src_row_base + offset) * 2) as u32;
                    let dst_addr = ((dst_row_base + offset) * 2) as u32;
                    writer.emit_copy16(dst_addr, src_addr, count, stats)?;
                    if self.color_depth_24 {
                        writer.emit_copy8(
                            base8 + (dst_row_base + offset) as u32,
                            base8 + (src_row_base + offset) as u32,
                            count,
                            stats,
                        )?;
                    }
                    offset += count;
                    remaining -= count;
                }
            }
            stats.pixels_copied_hw += mw as u64;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_dirty_rect_bgra<S: ChunkSink>(
        &mut self,
        surface: &[u8],
        stride_bytes: usize,
        rect: DirtyRect,
        force: bool,
        tier: QualityTier,
        interlace_parity: usize,
        writer: &mut ChunkWriter<'_, S>,
        stats: &mut FrameStats,
    ) -> Result<(), S::Error> {
        let fb_w = self.width as usize;
        let x0 = rect.left as usize;
        let x1 = rect.right as usize;
        let span_w = x1 - x0;
        let dither = self.dither && tier == QualityTier::Lossless;

        for y in rect.top as usize..rect.bottom as usize {
            // In MotionUltra, alternate even/odd scanlines every frame (60 Hz temporal fluidity,
            // 50% USB bandwidth!) and mark skipped lines for immediate refinement when motion slows.
            if tier == QualityTier::MotionUltra && (y & 1) != interlace_parity {
                self.mark_row_degraded(y);
                continue;
            }

            let src_row_off = y.saturating_mul(stride_bytes) + x0 * 4;
            let src_row_end = src_row_off + span_w * 4;
            if src_row_end > surface.len() {
                break;
            }
            let src_bytes = &surface[src_row_off..src_row_end];
            let scratch = &mut self.row_scratch[..span_w];

            match tier {
                QualityTier::Lossless => {
                    if dither {
                        for (i, px) in src_bytes.as_chunks::<4>().0.iter().enumerate() {
                            scratch[i] = bgra_to_rgb565_dither(px[0], px[1], px[2], x0 + i, y);
                        }
                    } else {
                        convert_bgra_row_rgb565(src_bytes, scratch, 0xFFFF);
                    }
                    if x0 == 0 && x1 == fb_w && self.row_degraded[y] {
                        self.row_degraded[y] = false;
                        self.degraded_row_count = self.degraded_row_count.saturating_sub(1);
                    }
                }
                QualityTier::MotionFast => {
                    convert_bgra_row_rgb565(src_bytes, scratch, 0xF7DE);
                }
                QualityTier::MotionUltra => {
                    // AVX2/SSE2 RGB444 conversion + horizontal 2-pixel RLE smoothing for extreme motion.
                    convert_bgra_row_rgb565(src_bytes, scratch, 0xF79E);
                    let mut i = 0usize;
                    while i + 1 < span_w {
                        let p0 = scratch[i];
                        let p1 = scratch[i + 1];
                        if rgb565_within_tolerance(p0, p1, 2) {
                            scratch[i + 1] = p0;
                        }
                        i += 2;
                    }
                }
            }

            stats.pixels_examined += span_w as u64;
            let shadow_row = &mut self.shadow_fb[y * fb_w + x0..y * fb_w + x1];

            if force {
                shadow_row.copy_from_slice(scratch);
                let dev_addr = ((y * fb_w + x0) * 2) as u32;
                writer.emit_span16(dev_addr, scratch, stats)?;
                stats.pixels_changed += span_w as u64;
                self.mark_row_base8_dirty(y);
                continue;
            }

            // Temporal micro-noise deadzone in motion tiers: suppress ±1 LSB shimmer from video codecs.
            let tol = match tier {
                QualityTier::Lossless => 0,
                QualityTier::MotionFast => 1,
                QualityTier::MotionUltra => 2,
            };

            let mut row_modified = false;
            let mut idx = 0usize;
            while idx < span_w {
                // AVX2 fast-skip identical 16-pixel (32-byte) blocks!
                idx = find_first_diff_u16(scratch, shadow_row, idx);
                if tol > 0 {
                    while idx < span_w
                        && (scratch[idx] == shadow_row[idx]
                            || rgb565_within_tolerance(scratch[idx], shadow_row[idx], tol))
                    {
                        idx += 1;
                    }
                }
                if idx >= span_w {
                    break;
                }

                let span_start = idx;
                let mut last_diff = idx;
                idx += 1;
                let mut same_run = 0usize;
                while idx < span_w {
                    let unchanged = scratch[idx] == shadow_row[idx]
                        || (tol > 0 && rgb565_within_tolerance(scratch[idx], shadow_row[idx], tol));
                    if unchanged {
                        same_run += 1;
                        if same_run >= MIN_UNCHANGED_GAP {
                            break;
                        }
                    } else {
                        same_run = 0;
                        last_diff = idx;
                    }
                    idx += 1;
                }
                let span_end = last_diff + 1;

                shadow_row[span_start..span_end].copy_from_slice(&scratch[span_start..span_end]);
                let dev_addr = ((y * fb_w + x0 + span_start) * 2) as u32;
                writer.emit_span16(dev_addr, &scratch[span_start..span_end], stats)?;
                stats.pixels_changed += (span_end - span_start) as u64;
                row_modified = true;
            }

            if row_modified {
                self.mark_row_base8_dirty(y);
                if tier != QualityTier::Lossless {
                    self.mark_row_degraded(y);
                }
            }
        }
        Ok(())
    }
}

/// Stateful streaming writer that packs DisplayLink commands directly into fixed USB chunks.
struct ChunkWriter<'a, S: ChunkSink> {
    sink: &'a mut S,
    pos: usize,
    has_chunk: bool,
    huffman_enabled: bool,
}

impl<'a, S: ChunkSink> ChunkWriter<'a, S> {
    fn new(sink: &'a mut S, huffman_enabled: bool) -> Self {
        Self {
            sink,
            pos: 0,
            has_chunk: false,
            huffman_enabled,
        }
    }

    fn ensure_space(&mut self, min_bytes: usize, stats: &mut FrameStats) -> Result<(), S::Error> {
        if !self.has_chunk {
            let _ = self.sink.acquire_chunk()?;
            self.pos = 0;
            self.has_chunk = true;
            return Ok(());
        }
        let chunk_len = self.sink.acquire_chunk()?.len();
        if chunk_len - self.pos < min_bytes {
            let chunk = self.sink.acquire_chunk()?;
            for b in &mut chunk[self.pos..chunk_len] {
                *b = UDL_MSG_BULK;
            }
            self.sink.submit_chunk(chunk_len)?;
            stats.bytes_emitted += chunk_len as u64;
            stats.chunks_submitted += 1;
            let _ = self.sink.acquire_chunk()?;
            self.pos = 0;
        }
        Ok(())
    }

    fn emit_raw_cmd(&mut self, bytes: &[u8], stats: &mut FrameStats) -> Result<(), S::Error> {
        self.ensure_space(bytes.len() + 1, stats)?;
        let buf = self.sink.acquire_chunk()?;
        let p = self.pos;
        buf[p..p + bytes.len()].copy_from_slice(bytes);
        self.pos = p + bytes.len();
        Ok(())
    }

    fn emit_copy_cmd(
        &mut self,
        opcode: u8,
        dst_addr: u32,
        src_addr: u32,
        pixel_count: usize,
        stats: &mut FrameStats,
    ) -> Result<(), S::Error> {
        debug_assert!((1..=MAX_CMD_PIXELS).contains(&pixel_count));
        self.ensure_space(COPY_CMD_BYTES, stats)?;
        let buf = self.sink.acquire_chunk()?;
        let p = self.pos;
        buf[p] = UDL_MSG_BULK;
        buf[p + 1] = opcode;
        buf[p + 2] = ((dst_addr >> 16) & 0xFF) as u8;
        buf[p + 3] = ((dst_addr >> 8) & 0xFF) as u8;
        buf[p + 4] = (dst_addr & 0xFF) as u8;
        buf[p + 5] = (pixel_count & 0xFF) as u8;
        buf[p + 6] = ((src_addr >> 16) & 0xFF) as u8;
        buf[p + 7] = ((src_addr >> 8) & 0xFF) as u8;
        buf[p + 8] = (src_addr & 0xFF) as u8;
        self.pos = p + COPY_CMD_BYTES;
        Ok(())
    }

    fn emit_copy16(
        &mut self,
        dst_addr: u32,
        src_addr: u32,
        pixel_count: usize,
        stats: &mut FrameStats,
    ) -> Result<(), S::Error> {
        self.emit_copy_cmd(UDL_CMD_WRITECOPY16, dst_addr, src_addr, pixel_count, stats)
    }

    fn emit_copy8(
        &mut self,
        dst_addr: u32,
        src_addr: u32,
        pixel_count: usize,
        stats: &mut FrameStats,
    ) -> Result<(), S::Error> {
        self.emit_copy_cmd(UDL_CMD_WRITECOPY8, dst_addr, src_addr, pixel_count, stats)
    }

    /// Encode a single <= 256 px `WRITERLX16` (`0xAF 0x6B`) command into `out` and return its length.
    #[inline]
    fn build_rlx16_cmd(dev_addr: u32, pixels: &[u16], out: &mut [u8]) -> usize {
        debug_assert!(!pixels.is_empty() && pixels.len() <= MAX_CMD_PIXELS);
        out[0] = UDL_MSG_BULK;
        out[1] = UDL_CMD_WRITERLX16;
        out[2] = ((dev_addr >> 16) & 0xFF) as u8;
        out[3] = ((dev_addr >> 8) & 0xFF) as u8;
        out[4] = (dev_addr & 0xFF) as u8;
        out[5] = (pixels.len() & 0xFF) as u8;
        let mut raw_count_idx = 6usize;
        let mut cmd = 7usize;
        let mut cursor = 0usize;
        let mut raw_start = 0usize;
        let total = pixels.len();

        while cursor < total {
            let run_start = cursor;
            let px = pixels[cursor];
            let be = px.to_be_bytes();
            out[cmd] = be[0];
            out[cmd + 1] = be[1];
            cmd += 2;
            cursor += 1;

            while cursor < total && pixels[cursor] == px {
                cursor += 1;
            }
            let run_len = cursor - run_start;
            if run_len > 1 {
                let raw_count = (run_start - raw_start) + 1;
                out[raw_count_idx] = (raw_count & 0xFF) as u8;
                out[cmd] = ((run_len - 1) & 0xFF) as u8;
                cmd += 1;
                raw_start = cursor;
                raw_count_idx = cmd;
                cmd += 1;
            }
        }
        if cursor > raw_start {
            out[raw_count_idx] = ((cursor - raw_start) & 0xFF) as u8;
        } else {
            cmd -= 1;
        }
        cmd
    }

    /// Encode a single <= 256 px `WRITERLX8` (`0xAF 0x63`) command into `out` and return its length.
    #[inline]
    fn build_rlx8_cmd(dev_addr: u32, pixels: &[u8], out: &mut [u8]) -> usize {
        debug_assert!(!pixels.is_empty() && pixels.len() <= MAX_CMD_PIXELS);
        out[0] = UDL_MSG_BULK;
        out[1] = UDL_CMD_WRITERLX8;
        out[2] = ((dev_addr >> 16) & 0xFF) as u8;
        out[3] = ((dev_addr >> 8) & 0xFF) as u8;
        out[4] = (dev_addr & 0xFF) as u8;
        out[5] = (pixels.len() & 0xFF) as u8;
        let mut raw_count_idx = 6usize;
        let mut cmd = 7usize;
        let mut cursor = 0usize;
        let mut raw_start = 0usize;
        let total = pixels.len();

        while cursor < total {
            let run_start = cursor;
            let px = pixels[cursor];
            out[cmd] = px;
            cmd += 1;
            cursor += 1;

            while cursor < total && pixels[cursor] == px {
                cursor += 1;
            }
            let run_len = cursor - run_start;
            if run_len > 1 {
                let raw_count = (run_start - raw_start) + 1;
                out[raw_count_idx] = (raw_count & 0xFF) as u8;
                out[cmd] = ((run_len - 1) & 0xFF) as u8;
                cmd += 1;
                raw_start = cursor;
                raw_count_idx = cmd;
                cmd += 1;
            }
        }
        if cursor > raw_start {
            out[raw_count_idx] = ((cursor - raw_start) & 0xFF) as u8;
        } else {
            cmd -= 1;
        }
        cmd
    }

    /// Emit a 16-bit pixel span, automatically selecting the smaller of `WRITERLX16` (`0x6B`)
    /// and hardware Huffman `WRITECOMP16` (`0x78`) per sub-chunk when Huffman is enabled.
    fn emit_span16(
        &mut self,
        mut dev_addr: u32,
        pixels: &[u16],
        stats: &mut FrameStats,
    ) -> Result<(), S::Error> {
        if !self.huffman_enabled {
            return self.emit_rlx16_span(dev_addr, pixels, stats);
        }

        let mut rlx_buf = [0u8; 640];
        let mut comp_buf = [0u8; 640];
        let mut cursor = 0usize;
        let total = pixels.len();

        while cursor < total {
            let n = MAX_CMD_PIXELS.min(total - cursor);
            let sub = &pixels[cursor..cursor + n];
            let rlx_len = Self::build_rlx16_cmd(dev_addr, sub, &mut rlx_buf);

            // Skip Huffman when RLE already achieved > ~8x compression (long flat runs > ~24 px).
            if rlx_len > (n >> 2) + 16 {
                let comp_len = encode_comp16_command(dev_addr, sub, &mut comp_buf);
                // Only pick WRITECOMP16 when strictly smaller than RLX16 AND within 504 B (< 512 B USB packet).
                if comp_len < rlx_len && comp_len <= 504 {
                    self.emit_raw_cmd(&comp_buf[..comp_len], stats)?;
                } else {
                    self.emit_raw_cmd(&rlx_buf[..rlx_len], stats)?;
                }
            } else {
                self.emit_raw_cmd(&rlx_buf[..rlx_len], stats)?;
            }

            dev_addr = dev_addr.wrapping_add((n * 2) as u32);
            cursor += n;
        }
        Ok(())
    }

    /// Emit an 8-bit `base8` (`RGB323`) span using `WRITERLX8` (`0x63`) or `WRITECOMP8` (`0x70`).
    fn emit_span8(
        &mut self,
        mut dev_addr: u32,
        pixels: &[u8],
        stats: &mut FrameStats,
    ) -> Result<(), S::Error> {
        let mut rlx_buf = [0u8; 384];
        let mut comp_buf = [0u8; 384];
        let mut cursor = 0usize;
        let total = pixels.len();

        while cursor < total {
            let n = MAX_CMD_PIXELS.min(total - cursor);
            let sub = &pixels[cursor..cursor + n];
            let rlx_len = Self::build_rlx8_cmd(dev_addr, sub, &mut rlx_buf);
            if self.huffman_enabled && rlx_len > (n / 2) + 12 {
                let comp_len = encode_comp8_command(dev_addr, sub, &mut comp_buf);
                if comp_len < rlx_len {
                    self.emit_raw_cmd(&comp_buf[..comp_len], stats)?;
                } else {
                    self.emit_raw_cmd(&rlx_buf[..rlx_len], stats)?;
                }
            } else {
                self.emit_raw_cmd(&rlx_buf[..rlx_len], stats)?;
            }
            dev_addr = dev_addr.wrapping_add(n as u32);
            cursor += n;
        }
        Ok(())
    }

    fn emit_rlx16_span(
        &mut self,
        mut dev_addr: u32,
        pixels: &[u16],
        stats: &mut FrameStats,
    ) -> Result<(), S::Error> {
        let mut cursor = 0usize;
        let total = pixels.len();

        while cursor < total {
            self.ensure_space(MIN_RLX_CMD_BYTES, stats)?;
            let buf = self.sink.acquire_chunk()?;
            let buf_len = buf.len();
            let mut cmd = self.pos;

            buf[cmd] = UDL_MSG_BULK;
            buf[cmd + 1] = UDL_CMD_WRITERLX16;
            buf[cmd + 2] = ((dev_addr >> 16) & 0xFF) as u8;
            buf[cmd + 3] = ((dev_addr >> 8) & 0xFF) as u8;
            buf[cmd + 4] = (dev_addr & 0xFF) as u8;
            let cmd_pixels_count_idx = cmd + 5;
            let mut raw_pixels_count_idx = cmd + 6;
            cmd += 7;

            let max_by_buf = (buf_len - 1 - cmd) / 2;
            let cmd_limit = MAX_CMD_PIXELS.min(total - cursor).min(max_by_buf);
            let cmd_start = cursor;
            let cmd_end = cursor + cmd_limit;
            let mut raw_start = cursor;

            while cursor < cmd_end {
                let run_start = cursor;
                let px = pixels[cursor];
                let be = px.to_be_bytes();
                buf[cmd] = be[0];
                buf[cmd + 1] = be[1];
                cmd += 2;
                cursor += 1;

                while cursor < cmd_end && pixels[cursor] == px {
                    cursor += 1;
                }

                let run_len = cursor - run_start;
                if run_len > 1 {
                    let raw_count = (run_start - raw_start) + 1;
                    buf[raw_pixels_count_idx] = (raw_count & 0xFF) as u8;
                    buf[cmd] = ((run_len - 1) & 0xFF) as u8;
                    cmd += 1;
                    raw_start = cursor;
                    raw_pixels_count_idx = cmd;
                    cmd += 1;
                }
            }

            if cursor > raw_start {
                let raw_count = cursor - raw_start;
                buf[raw_pixels_count_idx] = (raw_count & 0xFF) as u8;
            } else {
                cmd -= 1;
            }

            let encoded_pixels = cursor - cmd_start;
            buf[cmd_pixels_count_idx] = (encoded_pixels & 0xFF) as u8;
            dev_addr = dev_addr.wrapping_add((encoded_pixels * 2) as u32);
            self.pos = cmd;
        }

        Ok(())
    }

    fn finish(mut self, stats: &mut FrameStats) -> Result<(), S::Error> {
        if self.has_chunk && self.pos > 0 {
            let buf = self.sink.acquire_chunk()?;
            if self.pos < buf.len() {
                buf[self.pos] = UDL_MSG_BULK;
                self.pos += 1;
            }
            let len = self.pos;
            self.sink.submit_chunk(len)?;
            stats.bytes_emitted += len as u64;
            stats.chunks_submitted += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::huffman::{encode_load_huffman_table, HuffHardwareSim};
    use crate::regs::{
        UDL_CMD_FLUSH_PIPE, UDL_CMD_LOAD_DECOMP_TABLE, UDL_CMD_WRITECOMP16, UDL_CMD_WRITECOMP8,
        UDL_CMD_WRITECOPY8, UDL_CMD_WRITERAW16, UDL_CMD_WRITEREG, UDL_CMD_WRITERL8,
        UDL_CMD_WRITERLX8,
    };

    struct Dl1x5HardwareSim {
        vram: Vec<u16>,
        vram8: Vec<u8>,
        base8_offset: usize,
        regs: [u8; 256],
        huff: Option<HuffHardwareSim>,
    }

    impl Dl1x5HardwareSim {
        fn new(width: usize, height: usize) -> Self {
            Self {
                vram: vec![0u16; width * height],
                vram8: vec![0u8; width * height],
                base8_offset: width * height * 2,
                regs: [0u8; 256],
                huff: None,
            }
        }

        fn reconstruct_rgb888(&self) -> Vec<(u8, u8, u8)> {
            self.vram
                .iter()
                .zip(self.vram8.iter())
                .map(|(&p16, &p8)| {
                    let r = (((p16 >> 8) & 0xF8) as u8) | ((p8 >> 5) & 0x07);
                    let g = (((p16 >> 3) & 0xFC) as u8) | ((p8 >> 3) & 0x03);
                    let b = (((p16 << 3) & 0xF8) as u8) | (p8 & 0x07);
                    (r, g, b)
                })
                .collect()
        }

        fn execute(&mut self, stream: &[u8]) {
            let mut i = 0usize;
            while i < stream.len() {
                assert_eq!(stream[i], UDL_MSG_BULK);
                i += 1;
                if i >= stream.len() {
                    break;
                }
                if stream[i] == UDL_MSG_BULK {
                    continue;
                }
                let cmd = stream[i];
                i += 1;
                match cmd {
                    UDL_CMD_FLUSH_PIPE => {}
                    UDL_CMD_LOAD_DECOMP_TABLE => {
                        let count =
                            u32::from_be_bytes([stream[i + 4], stream[i + 5], stream[i + 6], stream[i + 7]])
                                as usize;
                        let payload_len = 8 + count * 9;
                        self.huff = Some(HuffHardwareSim::from_e0_payload(&stream[i..i + payload_len]));
                        i += payload_len;
                    }
                    UDL_CMD_WRITEREG => {
                        self.regs[stream[i] as usize] = stream[i + 1];
                        i += 2;
                    }
                    UDL_CMD_WRITERAW16 => {
                        let addr = ((stream[i] as usize) << 16)
                            | ((stream[i + 1] as usize) << 8)
                            | (stream[i + 2] as usize);
                        let count = if stream[i + 3] == 0 { 256 } else { stream[i + 3] as usize };
                        i += 4;
                        let base = addr / 2;
                        for k in 0..count {
                            self.vram[base + k] = u16::from_be_bytes([stream[i], stream[i + 1]]);
                            i += 2;
                        }
                    }
                    UDL_CMD_WRITECOPY16 => {
                        let dst = ((stream[i] as usize) << 16)
                            | ((stream[i + 1] as usize) << 8)
                            | (stream[i + 2] as usize);
                        let count = if stream[i + 3] == 0 { 256 } else { stream[i + 3] as usize };
                        let src = ((stream[i + 4] as usize) << 16)
                            | ((stream[i + 5] as usize) << 8)
                            | (stream[i + 6] as usize);
                        i += 7;
                        let tmp = self.vram[src / 2..src / 2 + count].to_vec();
                        self.vram[dst / 2..dst / 2 + count].copy_from_slice(&tmp);
                    }
                    UDL_CMD_WRITECOPY8 => {
                        let dst = ((stream[i] as usize) << 16)
                            | ((stream[i + 1] as usize) << 8)
                            | (stream[i + 2] as usize);
                        let count = if stream[i + 3] == 0 { 256 } else { stream[i + 3] as usize };
                        let src = ((stream[i + 4] as usize) << 16)
                            | ((stream[i + 5] as usize) << 8)
                            | (stream[i + 6] as usize);
                        i += 7;
                        let s = src - self.base8_offset;
                        let d = dst - self.base8_offset;
                        let tmp = self.vram8[s..s + count].to_vec();
                        self.vram8[d..d + count].copy_from_slice(&tmp);
                    }
                    UDL_CMD_WRITERL8 => {
                        let addr = ((stream[i] as usize) << 16)
                            | ((stream[i + 1] as usize) << 8)
                            | (stream[i + 2] as usize);
                        let total = if stream[i + 3] == 0 { 256 } else { stream[i + 3] as usize };
                        i += 4;
                        let mut out_idx = addr - self.base8_offset;
                        let mut written = 0usize;
                        while written < total {
                            let run = if stream[i] == 0 { 256 } else { stream[i] as usize };
                            let val = stream[i + 1];
                            i += 2;
                            for _ in 0..run {
                                self.vram8[out_idx] = val;
                                out_idx += 1;
                                written += 1;
                            }
                        }
                    }
                    UDL_CMD_WRITERLX8 => {
                        let addr = ((stream[i] as usize) << 16)
                            | ((stream[i + 1] as usize) << 8)
                            | (stream[i + 2] as usize);
                        let total = if stream[i + 3] == 0 { 256 } else { stream[i + 3] as usize };
                        i += 4;
                        let mut out_idx = addr - self.base8_offset;
                        let mut written = 0usize;
                        while written < total {
                            let raw_cnt = if stream[i] == 0 { 256 } else { stream[i] as usize };
                            i += 1;
                            let mut last_px = 0u8;
                            for _ in 0..raw_cnt {
                                last_px = stream[i];
                                i += 1;
                                self.vram8[out_idx] = last_px;
                                out_idx += 1;
                                written += 1;
                            }
                            if written < total {
                                let rep_cnt = if stream[i] == 0 { 256 } else { stream[i] as usize };
                                i += 1;
                                for _ in 0..rep_cnt {
                                    self.vram8[out_idx] = last_px;
                                    out_idx += 1;
                                    written += 1;
                                }
                            }
                        }
                    }
                    UDL_CMD_WRITERLX16 => {
                        let addr = ((stream[i] as usize) << 16)
                            | ((stream[i + 1] as usize) << 8)
                            | (stream[i + 2] as usize);
                        let total = if stream[i + 3] == 0 { 256 } else { stream[i + 3] as usize };
                        i += 4;
                        let mut out_idx = addr / 2;
                        let mut written = 0usize;
                        while written < total {
                            let raw_cnt = if stream[i] == 0 { 256 } else { stream[i] as usize };
                            i += 1;
                            let mut last_px = 0u16;
                            for _ in 0..raw_cnt {
                                last_px = u16::from_be_bytes([stream[i], stream[i + 1]]);
                                i += 2;
                                self.vram[out_idx] = last_px;
                                out_idx += 1;
                                written += 1;
                            }
                            if written < total {
                                let rep_cnt = if stream[i] == 0 { 256 } else { stream[i] as usize };
                                i += 1;
                                for _ in 0..rep_cnt {
                                    self.vram[out_idx] = last_px;
                                    out_idx += 1;
                                    written += 1;
                                }
                            }
                        }
                    }
                    UDL_CMD_WRITECOMP16 => {
                        let addr = ((stream[i] as usize) << 16)
                            | ((stream[i + 1] as usize) << 8)
                            | (stream[i + 2] as usize);
                        let total = if stream[i + 3] == 0 { 256 } else { stream[i + 3] as usize };
                        i += 4;
                        let huff = self.huff.as_ref().expect("Huffman table must be loaded first");
                        let (decoded, consumed) = huff.decode_comp16(&stream[i..], total);
                        let base = addr / 2;
                        self.vram[base..base + total].copy_from_slice(&decoded);
                        i += consumed;
                    }
                    UDL_CMD_WRITECOMP8 => {
                        let addr = ((stream[i] as usize) << 16)
                            | ((stream[i + 1] as usize) << 8)
                            | (stream[i + 2] as usize);
                        let total = if stream[i + 3] == 0 { 256 } else { stream[i + 3] as usize };
                        i += 4;
                        let huff = self.huff.as_ref().expect("Huffman table must be loaded first");
                        let (decoded, consumed) = huff.decode_comp8(&stream[i..], total);
                        let base = addr - self.base8_offset;
                        self.vram8[base..base + total].copy_from_slice(&decoded);
                        i += consumed;
                    }
                    other => panic!("Unexpected opcode {other:#04X}"),
                }
            }
        }
    }

    fn expected_rgb565(surface: &[u8], w: usize, h: usize) -> Vec<u16> {
        let mut out = Vec::with_capacity(w * h);
        for px in surface.as_chunks::<4>().0 {
            out.push(bgra_to_rgb565(px[0], px[1], px[2]));
        }
        out
    }

    #[test]
    fn rlx16_roundtrip_mixed_patterns_and_chunk_boundaries() {
        let (w, h) = (320usize, 120usize);
        let mut surface = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 4;
                let (r, g, b) = if y < 30 {
                    (0x20, 0x40, 0x80)
                } else if y < 60 {
                    (((x / 2) * 7) as u8, ((y * 5) as u8), (x as u8))
                } else if y < 90 {
                    let v = ((x * 37 + y * 91) ^ (x >> 2)) as u8;
                    (v, v.wrapping_mul(3), v.wrapping_add(17))
                } else if x % 17 < 12 {
                    (255, 255, 255)
                } else {
                    (10, 20, 30)
                };
                surface[off] = b;
                surface[off + 1] = g;
                surface[off + 2] = r;
                surface[off + 3] = 255;
            }
        }

        let mut enc = FrameEncoder::new(w as u32, h as u32);
        let mut sink = VecChunkSink::new(512);
        let stats = enc.encode_frame_bgra(&surface, w * 4, &[], &[], &mut sink).unwrap();
        assert_eq!(stats.pixels_changed, (w * h) as u64);

        let mut sim = Dl1x5HardwareSim::new(w, h);
        for chunk in &sink.submitted {
            sim.execute(chunk);
        }
        assert_eq!(sim.vram, expected_rgb565(&surface, w, h));
    }

    #[test]
    fn incremental_diff_and_hardware_move_region_roundtrip() {
        let (w, h) = (128usize, 64usize);
        let mut surface = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 4;
                surface[off] = (x * 2) as u8;
                surface[off + 1] = (y * 4) as u8;
                surface[off + 2] = ((x + y) * 3) as u8;
                surface[off + 3] = 255;
            }
        }

        let mut enc = FrameEncoder::new(w as u32, h as u32);
        let mut sim = Dl1x5HardwareSim::new(w, h);

        let mut sink = VecChunkSink::new(4096);
        enc.encode_frame_bgra(&surface, w * 4, &[], &[], &mut sink).unwrap();
        for c in &sink.submitted {
            sim.execute(c);
        }
        assert_eq!(sim.vram, expected_rgb565(&surface, w, h));

        let mut next = surface.clone();
        for y in (10..50).rev() {
            let src_off = y * w * 4;
            let dst_off = (y + 8) * w * 4;
            next.copy_within(src_off..src_off + w * 4, dst_off);
        }
        for y in 10..18 {
            for x in 10..90 {
                let off = (y * w + x) * 4;
                next[off] = 0x11;
                next[off + 1] = 0xEE;
                next[off + 2] = 0x55;
            }
        }

        let mv = MoveRegion {
            src_x: 0,
            src_y: 10,
            dst_x: 0,
            dst_y: 18,
            width: w as u32,
            height: 40,
        };
        let dirty = DirtyRect {
            left: 0,
            top: 10,
            right: w as u32,
            bottom: 18,
        };

        let mut sink2 = VecChunkSink::new(4096);
        let stats2 = enc.encode_frame_bgra(&next, w * 4, &[mv], &[dirty], &mut sink2).unwrap();
        assert_eq!(stats2.pixels_copied_hw, (w * 40) as u64);
        for c in &sink2.submitted {
            sim.execute(c);
        }
        assert_eq!(sim.vram, expected_rgb565(&next, w, h));
    }

    #[test]
    fn adaptive_motion_ultra_saves_bandwidth_and_refines_to_bit_exact_lossless() {
        let (w, h) = (256usize, 128usize);
        let mut surface = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 4;
                // Smooth natural-like gradient + subtle detail
                surface[off] = ((x * 255) / w) as u8;
                surface[off + 1] = ((y * 255) / h) as u8;
                surface[off + 2] = (((x + y) * 127) / w) as u8;
                surface[off + 3] = 255;
            }
        }

        let mut enc = FrameEncoder::new(w as u32, h as u32);
        let mut sim = Dl1x5HardwareSim::new(w, h);

        // Initial black frame
        let black = vec![0u8; w * h * 4];
        let mut sink0 = VecChunkSink::new(4096);
        enc.encode_frame_bgra(&black, w * 4, &[], &[], &mut sink0).unwrap();
        for c in &sink0.submitted {
            sim.execute(c);
        }

        // Send smooth gradient frame in MotionUltra tier -> huge bandwidth reduction!
        let full = [DirtyRect { left: 0, top: 0, right: w as u32, bottom: h as u32 }];
        let mut sink_ultra = VecChunkSink::new(4096);
        let stats_ultra = enc
            .encode_frame_bgra_adaptive(&surface, w * 4, &[], &full, QualityTier::MotionUltra, &mut sink_ultra)
            .unwrap();
        for c in &sink_ultra.submitted {
            sim.execute(c);
        }
        assert!(enc.needs_refinement());
        assert!(
            stats_ultra.bytes_emitted < (w * h) as u64 / 2,
            "MotionUltra should drastically shrink smooth gradients: {} bytes",
            stats_ultra.bytes_emitted
        );

        // Idle static refinement pass -> restores 100% bit-exact RGB565!
        let mut sink_ref = VecChunkSink::new(4096);
        enc.encode_frame_bgra_adaptive(&surface, w * 4, &[], &[], QualityTier::Lossless, &mut sink_ref)
            .unwrap();
        for c in &sink_ref.submitted {
            sim.execute(c);
        }
        assert!(!enc.needs_refinement());
        assert_eq!(sim.vram, expected_rgb565(&surface, w, h));
    }

    #[test]
    fn simd_row_conversion_matches_scalar_for_all_lengths_and_masks() {
        for len in 0..=65usize {
            let mut src = vec![0u8; len * 4];
            for i in 0..len {
                src[i * 4] = ((i * 19 + 7) & 0xFF) as u8;
                src[i * 4 + 1] = ((i * 37 + 13) & 0xFF) as u8;
                src[i * 4 + 2] = ((i * 73 + 29) & 0xFF) as u8;
                src[i * 4 + 3] = 0xFF;
            }
            for mask in [0xFFFFu16, 0xF7DE, 0xF79E] {
                let mut simd_out = vec![0u16; len];
                convert_bgra_row_rgb565(&src, &mut simd_out, mask);
                for i in 0..len {
                    let expected = bgra_to_rgb565(src[i * 4], src[i * 4 + 1], src[i * 4 + 2]) & mask;
                    assert_eq!(simd_out[i], expected, "mismatch at len={len} i={i} mask={mask:#06X}");
                }
            }
        }
    }

    #[test]
    fn huffman_and_24bit_static_refinement_roundtrip_bit_exact_rgb888() {
        let (w, h) = (320usize, 120usize);
        let mut surface = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 4;
                // Smooth 24-bit color gradient with fine 1-step LSB variation
                let r = ((x * 255) / w) as u8;
                let g = ((y * 255) / h) as u8;
                let b = (((x + y * 2) * 255) / (w + h * 2)) as u8;
                surface[off] = b;
                surface[off + 1] = g;
                surface[off + 2] = r;
                surface[off + 3] = 255;
            }
        }

        // Measure RLX16-only size vs Hardware Huffman size on the exact same gradient!
        let mut enc_rlx = FrameEncoder::new(w as u32, h as u32);
        let mut sink_rlx = VecChunkSink::new(8192);
        let stats_rlx = enc_rlx.encode_frame_bgra(&surface, w * 4, &[], &[], &mut sink_rlx).unwrap();

        let mut enc = FrameEncoder::new(w as u32, h as u32);
        enc.set_huffman_enabled(true);
        enc.set_color_depth_24(true);

        let mut sim = Dl1x5HardwareSim::new(w, h);
        let mut init_cmd = Vec::new();
        encode_load_huffman_table(&mut init_cmd);
        sim.execute(&init_cmd);

        let mut sink = VecChunkSink::new(8192);
        let stats_huff = enc.encode_frame_bgra(&surface, w * 4, &[], &[], &mut sink).unwrap();
        for c in &sink.submitted {
            sim.execute(c);
        }
        // Huffman must be strictly lossless on RGB565 AND significantly smaller than RLX16 on smooth gradients!
        assert_eq!(sim.vram, expected_rgb565(&surface, w, h));
        assert!(
            stats_huff.bytes_emitted < stats_rlx.bytes_emitted * 65 / 100,
            "expected Huffman ({} B) < 65% of RLX16 ({} B)",
            stats_huff.bytes_emitted,
            stats_rlx.bytes_emitted
        );

        // Static refinement pass upgrades base8 to 100% bit-exact 24-bit RGB888!
        assert!(enc.needs_refinement());
        let mut sink_ref = VecChunkSink::new(8192);
        enc.encode_frame_bgra_adaptive(&surface, w * 4, &[], &[], QualityTier::Lossless, &mut sink_ref)
            .unwrap();
        for c in &sink_ref.submitted {
            sim.execute(c);
        }
        assert!(!enc.needs_refinement());

        let expected24: Vec<(u8, u8, u8)> = surface
            .as_chunks::<4>()
            .0
            .iter()
            .map(|px| (px[2], px[1], px[0]))
            .collect();
        assert_eq!(sim.reconstruct_rgb888(), expected24);
    }
}
