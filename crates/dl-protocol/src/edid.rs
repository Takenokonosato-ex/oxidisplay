//! EDID 1.3 validation and Detailed Timing Descriptor (DTD) parser,
//! plus fallback EDID for the Wacom DTU-1031 (1280x800 @ 60Hz).

use crate::regs::VideoMode;

pub const EDID_BLOCK_SIZE: usize = 128;

/// Genuine 128-byte EDID from Wacom DTU-1031 (WAC1039, 1280x800 @ 60.002 Hz, 71.11 MHz).
/// Used only as fallback if hardware I2C EDID read fails.
pub const DTU_1031_FALLBACK_EDID: [u8; EDID_BLOCK_SIZE] = [
    0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00,
    0x5C, 0x23, 0x39, 0x10, 0x01, 0x00, 0x00, 0x00,
    0x24, 0x16, 0x01, 0x03, 0x81, 0x15, 0x0D, 0x78,
    0x0A, 0x86, 0xB5, 0x97, 0x57, 0x4F, 0x93, 0x26,
    0x20, 0x50, 0x54, 0x00, 0x00, 0x00, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0xC7, 0x1B,
    0x00, 0xA0, 0x50, 0x20, 0x17, 0x30, 0x30, 0x20,
    0x36, 0x00, 0xD9, 0x88, 0x00, 0x00, 0x00, 0x1E,
    0x00, 0x00, 0x00, 0xFD, 0x00, 0x36, 0x3D, 0x2C,
    0x33, 0x08, 0x00, 0x0A, 0x20, 0x20, 0x20, 0x20,
    0x20, 0x20, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x31,
    0x0A, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20,
    0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0xFC,
    0x00, 0x44, 0x54, 0x55, 0x2D, 0x31, 0x30, 0x33,
    0x31, 0x0A, 0x20, 0x20, 0x20, 0x20, 0x00, 0xF7,
];

/// Parsed summary of an EDID base block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEdid {
    pub manufacturer_id: [u8; 3],
    pub product_code: u16,
    pub monitor_name: String,
    pub width_cm: u8,
    pub height_cm: u8,
    pub modes: Vec<VideoMode>,
}

/// Validate standard 8-byte EDID header and modulo-256 checksum on block 0.
pub fn is_valid_edid(edid: &[u8]) -> bool {
    if edid.len() < EDID_BLOCK_SIZE {
        return false;
    }
    const HEADER: [u8; 8] = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if edid[..8] != HEADER {
        return false;
    }
    let sum: u8 = edid[..EDID_BLOCK_SIZE]
        .iter()
        .fold(0u8, |acc, &b| acc.wrapping_add(b));
    sum == 0
}

/// Parse an EDID 1.3 base block (128 bytes) into monitor info and Detailed Timing modes.
pub fn parse_edid(edid: &[u8]) -> Option<ParsedEdid> {
    if !is_valid_edid(edid) {
        return None;
    }

    let mfg_raw = u16::from_be_bytes([edid[8], edid[9]]);
    let manufacturer_id = [
        (((mfg_raw >> 10) & 0x1F) as u8).saturating_add(b'@'),
        (((mfg_raw >> 5) & 0x1F) as u8).saturating_add(b'@'),
        ((mfg_raw & 0x1F) as u8).saturating_add(b'@'),
    ];
    let product_code = u16::from_le_bytes([edid[10], edid[11]]);
    let width_cm = edid[21];
    let height_cm = edid[22];

    let mut monitor_name = String::new();
    let mut modes = Vec::new();

    // 4 descriptor slots starting at byte 54, each 18 bytes.
    for slot in 0..4 {
        let d = &edid[54 + slot * 18..54 + (slot + 1) * 18];
        let pix_clk_10khz = u16::from_le_bytes([d[0], d[1]]);
        if pix_clk_10khz != 0 {
            if let Some(mode) = parse_dtd(d) {
                modes.push(mode);
            }
        } else if d[2] == 0 && d[3] == 0xFC && d[4] == 0 {
            // Monitor Name ASCII descriptor (13 bytes, padded with 0x0A / 0x20)
            let raw = &d[5..18];
            let end = raw.iter().position(|&b| b == 0x0A || b == 0x00).unwrap_or(raw.len());
            if let Ok(s) = core::str::from_utf8(&raw[..end]) {
                monitor_name = s.trim().to_string();
            }
        }
    }

    Some(ParsedEdid {
        manufacturer_id,
        product_code,
        monitor_name,
        width_cm,
        height_cm,
        modes,
    })
}

/// Parse an 18-byte EDID Detailed Timing Descriptor into a `VideoMode`.
pub fn parse_dtd(d: &[u8]) -> Option<VideoMode> {
    if d.len() < 18 {
        return None;
    }
    let clock_10khz = u16::from_le_bytes([d[0], d[1]]) as u32;
    if clock_10khz == 0 {
        return None;
    }

    let hactive = (d[2] as u16) | (((d[4] & 0xF0) as u16) << 4);
    let hblank = (d[3] as u16) | (((d[4] & 0x0F) as u16) << 8);
    let vactive = (d[5] as u16) | (((d[7] & 0xF0) as u16) << 4);
    let vblank = (d[6] as u16) | (((d[7] & 0x0F) as u16) << 8);

    let hsync_offset = (d[8] as u16) | (((d[11] & 0xC0) as u16) << 2);
    let hsync_pulse = (d[9] as u16) | (((d[11] & 0x30) as u16) << 4);
    let vsync_offset = ((d[10] >> 4) as u16) | (((d[11] & 0x0C) as u16) << 2);
    let vsync_pulse = ((d[10] & 0x0F) as u16) | (((d[11] & 0x03) as u16) << 4);

    if hactive == 0 || vactive == 0 || hblank == 0 || vblank == 0 {
        return None;
    }

    Some(VideoMode {
        hdisplay: hactive,
        hsync_start: hactive.saturating_add(hsync_offset),
        hsync_end: hactive.saturating_add(hsync_offset).saturating_add(hsync_pulse),
        htotal: hactive.saturating_add(hblank),
        vdisplay: vactive,
        vsync_start: vactive.saturating_add(vsync_offset),
        vsync_end: vactive.saturating_add(vsync_offset).saturating_add(vsync_pulse),
        vtotal: vactive.saturating_add(vblank),
        clock_khz: clock_10khz * 10,
    })
}
