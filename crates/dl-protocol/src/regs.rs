//! DisplayLink DL-1x5 (DL-125 / DL-165 / DL-195) hardware protocol registers,
//! LFSR16 timing generator, vendor descriptor parser, and register command builders.

/// Bulk command prefix byte (also acts as a single-byte NOP padding between commands).
pub const UDL_MSG_BULK: u8 = 0xAF;

/// Bulk opcodes
pub const UDL_CMD_WRITEREG: u8 = 0x20;
pub const UDL_CMD_WRITERAW8: u8 = 0x60;
pub const UDL_CMD_WRITERL8: u8 = 0x61;
pub const UDL_CMD_WRITECOPY8: u8 = 0x62;
pub const UDL_CMD_WRITERLX8: u8 = 0x63;
pub const UDL_CMD_WRITERAW16: u8 = 0x68;
pub const UDL_CMD_WRITERL16: u8 = 0x69;
pub const UDL_CMD_WRITECOPY16: u8 = 0x6A;
pub const UDL_CMD_WRITERLX16: u8 = 0x6B;

/// USB control transfer bRequest values (vendor-specific)
pub const USB_REQ_I2C_READ: u8 = 0x02;
pub const USB_REQ_SET_CHANNEL: u8 = 0x12;

/// USB vendor descriptor type for DisplayLink DL-1x5 capabilities.
pub const USB_DESC_TYPE_VENDOR_DL: u8 = 0x5F;

/// Standard 16-byte channel selection payload sent via vendor control OUT request 0x12.
pub const STD_CHANNEL_PAYLOAD: [u8; 16] = [
    0x57, 0xCD, 0xDC, 0xA7, 0x1C, 0x88, 0x5E, 0x15,
    0x60, 0xFE, 0xC6, 0x97, 0x16, 0x3D, 0x47, 0xF2,
];

/// Color depth register & values
pub const UDL_REG_COLORDEPTH: u8 = 0x00;
pub const UDL_COLORDEPTH_16BPP: u8 = 0x00;
pub const UDL_COLORDEPTH_24BPP: u8 = 0x01;

/// Display timing registers
pub const UDL_REG_XDISPLAYSTART: u8 = 0x01;
pub const UDL_REG_XDISPLAYEND: u8 = 0x03;
pub const UDL_REG_YDISPLAYSTART: u8 = 0x05;
pub const UDL_REG_YDISPLAYEND: u8 = 0x07;
pub const UDL_REG_XENDCOUNT: u8 = 0x09;
pub const UDL_REG_HSYNCSTART: u8 = 0x0B;
pub const UDL_REG_HSYNCEND: u8 = 0x0D;
pub const UDL_REG_HPIXELS: u8 = 0x0F;
pub const UDL_REG_YENDCOUNT: u8 = 0x11;
pub const UDL_REG_VSYNCSTART: u8 = 0x13;
pub const UDL_REG_VSYNCEND: u8 = 0x15;
pub const UDL_REG_VPIXELS: u8 = 0x17;
pub const UDL_REG_PIXELCLOCK5KHZ: u8 = 0x1B;

/// Blank / DPMS mode register
pub const UDL_REG_BLANKMODE: u8 = 0x1F;

/// Framebuffer base addresses (24-bit big-endian across 3 registers)
pub const UDL_REG_BASE16BPP_ADDR2: u8 = 0x20;
pub const UDL_REG_BASE16BPP_ADDR1: u8 = 0x21;
pub const UDL_REG_BASE16BPP_ADDR0: u8 = 0x22;
pub const UDL_REG_BASE8BPP_ADDR2: u8 = 0x26;
pub const UDL_REG_BASE8BPP_ADDR1: u8 = 0x27;
pub const UDL_REG_BASE8BPP_ADDR0: u8 = 0x28;

/// Video register lock/unlock
pub const UDL_REG_VIDREG: u8 = 0xFF;
pub const UDL_VIDREG_LOCK: u8 = 0x00;
pub const UDL_VIDREG_UNLOCK: u8 = 0xFF;

/// Default SKU pixel limit when vendor descriptor omits key 0x0200 (1920x1080+).
pub const DEFAULT_SKU_PIXEL_LIMIT: u32 = 2_080_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BlankMode {
    /// HSync and VSync active, framebuffer visible.
    On = 0x00,
    /// HSync and VSync active, output blanked.
    Blanked = 0x01,
    /// VSync off, blanked.
    VsyncOff = 0x03,
    /// HSync off, blanked.
    HsyncOff = 0x05,
    /// Powered down; requires a full modeset to wake.
    PowerDown = 0x07,
}

/// Complete CRT/panel video timing mode (matches DRM / EDID DTD conventions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoMode {
    pub hdisplay: u16,
    pub hsync_start: u16,
    pub hsync_end: u16,
    pub htotal: u16,
    pub vdisplay: u16,
    pub vsync_start: u16,
    pub vsync_end: u16,
    pub vtotal: u16,
    /// Pixel clock in kHz (e.g. 71_110 for 71.11 MHz).
    pub clock_khz: u32,
}

impl VideoMode {
    /// Native timing of the Wacom DTU-1031 (1280x800 @ 60.002 Hz, 71.11 MHz).
    pub const DTU_1031_1280X800_60: Self = Self {
        hdisplay: 1280,
        hsync_start: 1280 + 48,      // 1328
        hsync_end: 1280 + 48 + 32,   // 1360
        htotal: 1280 + 160,          // 1440
        vdisplay: 800,
        vsync_start: 800 + 3,        // 803
        vsync_end: 800 + 3 + 6,      // 809
        vtotal: 800 + 23,            // 823
        clock_khz: 71_110,
    };

    /// Pixel rate in Hz.
    #[inline]
    pub const fn pixel_rate_hz(&self) -> u64 {
        self.clock_khz as u64 * 1000
    }

    /// Horizontal sync frequency as (numerator, denominator) in Hz.
    #[inline]
    pub const fn hsync_freq(&self) -> (u32, u32) {
        (self.clock_khz * 1000, self.htotal as u32)
    }

    /// Vertical refresh frequency as (numerator, denominator) in Hz.
    #[inline]
    pub const fn vsync_freq(&self) -> (u32, u32) {
        let total_pixels = self.htotal as u32 * self.vtotal as u32;
        (self.clock_khz * 1000, total_pixels)
    }

    /// Refresh rate in millihertz (e.g. 60002 for 60.002 Hz).
    #[inline]
    pub const fn refresh_mhz(&self) -> u32 {
        let denom = self.htotal as u64 * self.vtotal as u64;
        if denom == 0 {
            return 0;
        }
        ((self.clock_khz as u64 * 1_000_000) / denom) as u32
    }

    /// Total active pixel count.
    #[inline]
    pub const fn pixel_count(&self) -> u32 {
        self.hdisplay as u32 * self.vdisplay as u32
    }
}

/// Parsed DisplayLink 0x5F vendor descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorDescriptor {
    pub version: u16,
    pub sku_pixel_limit: u32,
}

/// Parse the 0x5F vendor descriptor returned by DisplayLink DL-1x5 devices.
pub fn parse_vendor_descriptor(desc: &[u8]) -> Option<VendorDescriptor> {
    if desc.len() < 5 {
        return None;
    }
    let len = desc[0] as usize;
    if len > desc.len() || len < 5 || desc[1] != USB_DESC_TYPE_VENDOR_DL {
        return None;
    }
    let version = u16::from_le_bytes([desc[2], desc[3]]);
    if version != 0x0001 || desc[4] as usize != len - 2 {
        return None;
    }

    let mut sku_pixel_limit = DEFAULT_SKU_PIXEL_LIMIT;
    let mut pos = 5;
    while pos + 3 <= len {
        let key = u16::from_le_bytes([desc[pos], desc[pos + 1]]);
        let val_len = desc[pos + 2] as usize;
        pos += 3;
        if pos + val_len > len {
            break;
        }
        if key == 0x0200 && val_len >= 4 {
            let limit = u32::from_le_bytes([
                desc[pos],
                desc[pos + 1],
                desc[pos + 2],
                desc[pos + 3],
            ]);
            if limit > 0 && limit < 16 * DEFAULT_SKU_PIXEL_LIMIT {
                sku_pixel_limit = limit;
            }
        }
        pos += val_len;
    }

    Some(VendorDescriptor {
        version,
        sku_pixel_limit,
    })
}

/// Compute the 16-bit LFSR comparator value used by DisplayLink DL-1x5 timing counters.
///
/// The hardware starts from `0xFFFF` and shifts left each tick using polynomial
/// taps at bits 15, 4, 2, 1. Stepping `actual_count` iterations yields the comparator state.
#[inline]
pub const fn lfsr16(mut actual_count: u16) -> u16 {
    let mut lv: u32 = 0xFFFF;
    while actual_count > 0 {
        let feedback = ((lv >> 15) ^ (lv >> 4) ^ (lv >> 2) ^ (lv >> 1)) & 1;
        lv = ((lv << 1) | feedback) & 0xFFFF;
        actual_count -= 1;
    }
    lv as u16
}

#[inline]
pub fn emit_set_reg(buf: &mut Vec<u8>, reg: u8, val: u8) {
    buf.extend_from_slice(&[UDL_MSG_BULK, UDL_CMD_WRITEREG, reg, val]);
}

#[inline]
fn emit_set_reg16_be(buf: &mut Vec<u8>, reg: u8, val: u16) {
    emit_set_reg(buf, reg, (val >> 8) as u8);
    emit_set_reg(buf, reg + 1, (val & 0xFF) as u8);
}

#[inline]
fn emit_set_reg16_le(buf: &mut Vec<u8>, reg: u8, val: u16) {
    emit_set_reg(buf, reg, (val & 0xFF) as u8);
    emit_set_reg(buf, reg + 1, (val >> 8) as u8);
}

#[inline]
fn emit_set_reg_lfsr16(buf: &mut Vec<u8>, reg: u8, count: u16) {
    emit_set_reg16_be(buf, reg, lfsr16(count));
}

#[inline]
fn emit_set_base16bpp(buf: &mut Vec<u8>, base: u32) {
    emit_set_reg(buf, UDL_REG_BASE16BPP_ADDR2, ((base >> 16) & 0xFF) as u8);
    emit_set_reg(buf, UDL_REG_BASE16BPP_ADDR1, ((base >> 8) & 0xFF) as u8);
    emit_set_reg(buf, UDL_REG_BASE16BPP_ADDR0, (base & 0xFF) as u8);
}

#[inline]
fn emit_set_base8bpp(buf: &mut Vec<u8>, base: u32) {
    emit_set_reg(buf, UDL_REG_BASE8BPP_ADDR2, ((base >> 16) & 0xFF) as u8);
    emit_set_reg(buf, UDL_REG_BASE8BPP_ADDR1, ((base >> 8) & 0xFF) as u8);
    emit_set_reg(buf, UDL_REG_BASE8BPP_ADDR0, (base & 0xFF) as u8);
}

/// Emit a 1-pixel no-op copy command required by DL-1x5 to latch/flush register changes.
#[inline]
pub fn emit_dummy_render(buf: &mut Vec<u8>) {
    buf.extend_from_slice(&[
        UDL_MSG_BULK,
        UDL_CMD_WRITECOPY16,
        0x00, 0x00, 0x00, // dst addr
        0x01,             // 1 pixel
        0x00, 0x00, 0x00, // src addr
    ]);
}

/// Encode the complete bulk command sequence to configure a 16bpp RGB565 video mode
/// and enable display output.
pub fn encode_mode_set(mode: &VideoMode, buf: &mut Vec<u8>) {
    let reg01 = mode.htotal.saturating_sub(mode.hsync_start);
    let reg03 = reg01.saturating_add(mode.hdisplay);
    let reg05 = mode.vtotal.saturating_sub(mode.vsync_start);
    let reg07 = reg05.saturating_add(mode.vdisplay);
    let reg09 = mode.htotal.saturating_sub(1);
    let reg0b = 1u16; // libdlo / Linux udl hardcode HSync start to 1
    let reg0d = mode.hsync_end.saturating_sub(mode.hsync_start).saturating_add(1);
    let reg0f = mode.hdisplay;
    let reg11 = mode.vtotal;
    let reg13 = 0u16; // libdlo / Linux udl hardcode VSync start to 0
    let reg15 = mode.vsync_end.saturating_sub(mode.vsync_start);
    let reg17 = mode.vdisplay;
    let reg1b = (mode.clock_khz / 5) as u16;

    // Lock video registers before programming
    emit_set_reg(buf, UDL_REG_VIDREG, UDL_VIDREG_LOCK);
    emit_set_reg(buf, UDL_REG_COLORDEPTH, UDL_COLORDEPTH_16BPP);
    emit_set_base16bpp(buf, 0);
    emit_set_base8bpp(buf, 2 * mode.hdisplay as u32 * mode.vdisplay as u32);

    emit_set_reg_lfsr16(buf, UDL_REG_XDISPLAYSTART, reg01);
    emit_set_reg_lfsr16(buf, UDL_REG_XDISPLAYEND, reg03);
    emit_set_reg_lfsr16(buf, UDL_REG_YDISPLAYSTART, reg05);
    emit_set_reg_lfsr16(buf, UDL_REG_YDISPLAYEND, reg07);
    emit_set_reg_lfsr16(buf, UDL_REG_XENDCOUNT, reg09);
    emit_set_reg_lfsr16(buf, UDL_REG_HSYNCSTART, reg0b);
    emit_set_reg_lfsr16(buf, UDL_REG_HSYNCEND, reg0d);
    emit_set_reg16_be(buf, UDL_REG_HPIXELS, reg0f);
    emit_set_reg_lfsr16(buf, UDL_REG_YENDCOUNT, reg11);
    emit_set_reg_lfsr16(buf, UDL_REG_VSYNCSTART, reg13);
    emit_set_reg_lfsr16(buf, UDL_REG_VSYNCEND, reg15);
    emit_set_reg16_be(buf, UDL_REG_VPIXELS, reg17);
    // Pixel clock (in 5 kHz units) uses little-endian byte order across 0x1B/0x1C.
    emit_set_reg16_le(buf, UDL_REG_PIXELCLOCK5KHZ, reg1b);

    emit_set_reg(buf, UDL_REG_BLANKMODE, BlankMode::On as u8);
    emit_set_reg(buf, UDL_REG_VIDREG, UDL_VIDREG_UNLOCK);
    emit_dummy_render(buf);
}

/// Encode the bulk command sequence to set the display blank / power state.
pub fn encode_blank_mode(mode: BlankMode, buf: &mut Vec<u8>) {
    emit_set_reg(buf, UDL_REG_VIDREG, UDL_VIDREG_LOCK);
    emit_set_reg(buf, UDL_REG_BLANKMODE, mode as u8);
    emit_set_reg(buf, UDL_REG_VIDREG, UDL_VIDREG_UNLOCK);
    emit_dummy_render(buf);
}
