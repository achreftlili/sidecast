//! Optional full decoding of H.264 streams with OpenH264, to prove every
//! frame is valid beyond its NAL structure. HEVC is checked structurally only.

#[cfg(feature = "openh264")]
mod imp {
    use openh264::decoder::Decoder;
    use openh264::formats::YUVSource;

    pub struct FrameDecoder {
        decoder: Decoder,
    }

    impl FrameDecoder {
        pub fn new() -> Option<FrameDecoder> {
            Decoder::new().ok().map(|decoder| FrameDecoder { decoder })
        }

        /// The decoded picture size, `Ok(None)` while the decoder buffers.
        pub fn decode(&mut self, access_unit: &[u8]) -> Result<Option<(usize, usize)>, String> {
            match self.decoder.decode(access_unit) {
                Ok(Some(picture)) => Ok(Some(picture.dimensions())),
                Ok(None) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        }
    }
}

#[cfg(not(feature = "openh264"))]
mod imp {
    pub struct FrameDecoder;

    impl FrameDecoder {
        pub fn new() -> Option<FrameDecoder> {
            None
        }

        pub fn decode(&mut self, _access_unit: &[u8]) -> Result<Option<(usize, usize)>, String> {
            Ok(None)
        }
    }
}

pub use imp::FrameDecoder;
