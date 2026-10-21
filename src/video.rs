//! H264 RTP → frames → RGBA, for showing the live view in a window.

use bytes::{Bytes, BytesMut};
use rtc::rtp::codec::h264::H264Packet;
use rtc::rtp::packetizer::Depacketizer;

/// Rebuilds Annex-B access units from RTP payloads (single NAL, STAP-A,
/// FU-A), emitting one when the marker bit closes the frame.
#[derive(Default)]
pub struct FrameAssembler {
    depack: H264Packet,
    frame: BytesMut,
}

impl FrameAssembler {
    pub fn push(&mut self, payload: &Bytes, marker: bool) -> Option<Bytes> {
        if let Ok(nal) = self.depack.depacketize(payload) {
            self.frame.extend_from_slice(&nal);
        }
        if marker && !self.frame.is_empty() {
            return Some(self.frame.split().freeze());
        }
        None
    }
}

/// One decoded picture.
pub struct RgbaFrame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

#[cfg(feature = "gui")]
pub struct Decoder(openh264::decoder::Decoder);

#[cfg(feature = "gui")]
impl Decoder {
    pub fn new() -> Result<Self, String> {
        openh264::decoder::Decoder::new().map(Self).map_err(|e| e.to_string())
    }

    /// Decodes one access unit; `None` until a picture is complete.
    pub fn decode(&mut self, annexb: &[u8]) -> Option<RgbaFrame> {
        use openh264::formats::YUVSource;
        let yuv = self.0.decode(annexb).ok()??;
        let (width, height) = yuv.dimensions();
        let mut rgba = vec![0; width * height * 4];
        yuv.write_rgba8(&mut rgba);
        Some(RgbaFrame { width, height, rgba })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_nal_frame_is_emitted_on_marker() {
        let mut a = FrameAssembler::default();
        // SPS (type 7) then a slice (type 1) closing the frame.
        assert!(a.push(&Bytes::from_static(&[0x67, 0x42, 0x00, 0x1e]), false).is_none());
        let f = a.push(&Bytes::from_static(&[0x41, 0x9a, 0x02, 0x03]), true).unwrap();
        assert_eq!(&f[..], &[0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1e, 0, 0, 0, 1, 0x41, 0x9a, 0x02, 0x03]);
        assert!(a.push(&Bytes::from_static(&[0x41, 0x9a, 0x02, 0x03]), false).is_none());
    }

    #[test]
    fn fu_a_fragments_are_joined() {
        let mut a = FrameAssembler::default();
        // FU indicator 0x7c (type 28, NRI 3), FU headers: start / end, NAL type 5.
        assert!(a.push(&Bytes::from_static(&[0x7c, 0x85, 0xaa, 0xbb]), false).is_none());
        let f = a.push(&Bytes::from_static(&[0x7c, 0x45, 0xcc]), true).unwrap();
        assert_eq!(&f[..], &[0, 0, 0, 1, 0x65, 0xaa, 0xbb, 0xcc]);
    }
}
