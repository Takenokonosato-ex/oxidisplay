//! `dl-protocol`: Clean-room Rust implementation of the DisplayLink DL-1x5 USB protocol.

pub mod edid;
pub mod encoder;
pub mod regs;

pub use edid::{is_valid_edid, parse_edid, ParsedEdid, DTU_1031_FALLBACK_EDID, EDID_BLOCK_SIZE};
pub use encoder::{
    bgra_to_rgb565, bgra_to_rgb565_dither, ChunkSink, DirtyRect, FrameEncoder, FrameStats,
    MoveRegion, QualityTier, VecChunkSink, DEFAULT_USB_CHUNK_SIZE,
};
pub use regs::{
    encode_blank_mode, encode_mode_set, lfsr16, parse_vendor_descriptor, BlankMode,
    VendorDescriptor, VideoMode, DEFAULT_SKU_PIXEL_LIMIT, STD_CHANNEL_PAYLOAD, UDL_CMD_WRITECOPY16,
    UDL_CMD_WRITERAW16, UDL_CMD_WRITEREG, UDL_CMD_WRITERL16, UDL_CMD_WRITERLX16, UDL_MSG_BULK,
    USB_DESC_TYPE_VENDOR_DL, USB_REQ_I2C_READ, USB_REQ_SET_CHANNEL,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_real_dtu1031_vendor_descriptor() {
        // Real 0x5F descriptor captured from the connected Wacom DTU-1031 (VID_17E9&PID_04A3)
        let desc: [u8; 23] = [
            0x17, 0x5F, 0x01, 0x00, 0x15, 0x05, 0x00, 0x01, 0x03, 0x00, 0x04, 0x04, 0x01, 0x00,
            0x03, 0x60, 0x00, 0x02, 0x04, 0x60, 0xE3, 0x16, 0x00,
        ];
        let parsed = parse_vendor_descriptor(&desc).expect("valid DL-1x5 vendor descriptor");
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.sku_pixel_limit, 1_500_000);
    }

    #[test]
    fn parse_real_dtu1031_edid_and_encode_modeset() {
        let parsed = parse_edid(&DTU_1031_FALLBACK_EDID).expect("valid DTU-1031 EDID");
        assert_eq!(&parsed.manufacturer_id, b"WAC");
        assert_eq!(parsed.product_code, 0x1039);
        assert_eq!(parsed.monitor_name, "DTU-1031");
        assert_eq!(parsed.modes.len(), 1);

        let m = parsed.modes[0];
        assert_eq!(m, VideoMode::DTU_1031_1280X800_60);
        assert_eq!(m.hdisplay, 1280);
        assert_eq!(m.vdisplay, 800);
        assert_eq!(m.clock_khz, 71_110);
        assert_eq!(m.refresh_mhz(), 60_002);

        let mut buf = Vec::new();
        encode_mode_set(&m, &mut buf);
        // Should begin with vidreg lock (0xAF 0x20 0xFF 0x00) and end with dummy_render (0xAF 0x6A ...)
        assert_eq!(&buf[..4], &[0xAF, 0x20, 0xFF, 0x00]);
        assert_eq!(&buf[buf.len() - 9..], &[0xAF, 0x6A, 0, 0, 0, 1, 0, 0, 0]);
    }

    #[test]
    fn lfsr16_known_vectors() {
        assert_eq!(lfsr16(0), 0xFFFF);
        assert_eq!(lfsr16(1), 0xFFFE);
        // Known value for 1280x800 HTotal-1 = 1439 should be deterministic and non-zero
        assert_ne!(lfsr16(1439), 0);
        assert_ne!(lfsr16(1439), 0xFFFF);
    }
}
