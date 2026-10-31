//! Optional full decoding of H.264 streams with OpenH264, to prove every
//! frame is valid beyond its NAL structure. HEVC is checked structurally only.

#[cfg(feature = "openh264")]
mod imp {
    use openh264::decoder::Decoder;
    use openh264::formats::YUVSource;

    pub struct FrameDecoder {
        decoder: Decoder,
        keep_luma: bool,
        /// The last picture's luma, tightly packed (with `keep_luma`).
        last_luma: Option<(usize, usize, Vec<u8>)>,
    }

    impl FrameDecoder {
        pub fn new(keep_luma: bool) -> Option<FrameDecoder> {
            Decoder::new().ok().map(|decoder| FrameDecoder { decoder, keep_luma, last_luma: None })
        }

        /// The decoded picture size, `Ok(None)` while the decoder buffers.
        pub fn decode(&mut self, access_unit: &[u8]) -> Result<Option<(usize, usize)>, String> {
            match self.decoder.decode(access_unit) {
                Ok(Some(picture)) => {
                    let (w, h) = picture.dimensions();
                    if self.keep_luma {
                        let stride = picture.strides().0;
                        let luma = picture.y().chunks(stride).take(h).flat_map(|row| &row[..w]).copied().collect();
                        self.last_luma = Some((w, h, luma));
                    }
                    Ok(Some((w, h)))
                }
                Ok(None) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        }

        pub fn last_luma(&self) -> Option<&(usize, usize, Vec<u8>)> {
            self.last_luma.as_ref()
        }
    }
}

#[cfg(not(feature = "openh264"))]
mod imp {
    pub struct FrameDecoder;

    impl FrameDecoder {
        pub fn new(_keep_luma: bool) -> Option<FrameDecoder> {
            None
        }

        pub fn decode(&mut self, _access_unit: &[u8]) -> Result<Option<(usize, usize)>, String> {
            Ok(None)
        }

        pub fn last_luma(&self) -> Option<&(usize, usize, Vec<u8>)> {
            None
        }
    }
}

pub use imp::FrameDecoder;
