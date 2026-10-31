//! Media Foundation encoder output → the access units PROTOCOL §6 wants.
//! Pure, so it is tested on every OS.
//!
//! H.264 encoder MFTs emit Annex-B, but vendors differ in the details: some
//! use 3-byte start codes, some put SPS/PPS only in the media type's
//! sequence header (`MF_MT_MPEG_SEQUENCE_HEADER`) instead of before every
//! IDR. Every VIDEO_FRAME gets 4-byte start codes, and every keyframe its
//! parameter sets.

use sidecast_core::annexb::{self, START_CODE};

const NAL_IDR: u8 = 5;
const NAL_SPS: u8 = 7;
const NAL_PPS: u8 = 8;

/// One normalized access unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessUnit {
    pub data: Vec<u8>,
    /// Contains an IDR slice.
    pub is_keyframe: bool,
}

/// Normalizes one encoder output sample. `sequence_header` is the stream's
/// SPS/PPS in Annex-B (empty when unknown). `None` when the sample holds no
/// NAL unit at all.
pub fn normalize(data: &[u8], sequence_header: &[u8]) -> Option<AccessUnit> {
    let converted;
    let data = if data.starts_with(&[0, 0, 1]) || data.starts_with(&START_CODE) {
        data
    } else {
        // Not Annex-B: try 4-byte length prefixes (AVCC), which a few MFTs emit.
        converted = annexb::from_length_prefixed(data, 4, &[])?;
        &converted
    };
    let units = annexb::split(data);
    if units.is_empty() {
        return None;
    }
    let types: Vec<u8> = units.iter().filter_map(|u| annexb::h264_nal_type(u)).collect();
    let is_keyframe = types.contains(&NAL_IDR);
    let mut out = Vec::with_capacity(data.len() + sequence_header.len() + 16);
    if is_keyframe && !(types.contains(&NAL_SPS) && types.contains(&NAL_PPS)) {
        for unit in annexb::split(sequence_header) {
            if matches!(annexb::h264_nal_type(unit), Some(NAL_SPS | NAL_PPS)) {
                out.extend_from_slice(&START_CODE);
                out.extend_from_slice(unit);
            }
        }
    }
    for unit in units {
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(unit);
    }
    Some(AccessUnit { data: out, is_keyframe })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidecast_core::annexb::inspect_access_unit;
    use sidecast_core::streaming::VideoCodec;

    const SPS: &[u8] = &[0x67, 0x4D, 0x40, 0x1F, 0xAB];
    const PPS: &[u8] = &[0x68, 0xEE, 0x3C, 0x80];
    const IDR: &[u8] = &[0x65, 0x88, 0x84, 0x00, 0x21];
    const SLICE: &[u8] = &[0x41, 0x9A, 0x02, 0x03];

    fn annexb(units: &[&[u8]], start: &[u8]) -> Vec<u8> {
        units.iter().flat_map(|u| start.iter().chain(u.iter()).copied()).collect()
    }

    #[test]
    fn keyframes_get_their_parameter_sets_from_the_sequence_header() {
        let header = annexb(&[SPS, PPS], &START_CODE);
        let unit = normalize(&annexb(&[IDR], &[0, 0, 1]), &header).unwrap();
        assert!(unit.is_keyframe);
        assert_eq!(unit.data, annexb(&[SPS, PPS, IDR], &START_CODE));
        let info = inspect_access_unit(VideoCodec::H264, &unit.data, true).unwrap();
        assert!(info.has_parameter_sets && info.has_random_access_slice);
    }

    #[test]
    fn complete_keyframes_and_other_frames_only_get_4_byte_start_codes() {
        let full = annexb(&[SPS, PPS, IDR], &[0, 0, 1]);
        let unit = normalize(&full, &annexb(&[SPS, PPS], &START_CODE)).unwrap();
        assert_eq!(unit.data, annexb(&[SPS, PPS, IDR], &START_CODE), "no duplicated parameter sets");

        let p = normalize(&annexb(&[SLICE], &START_CODE), &[]).unwrap();
        assert!(!p.is_keyframe);
        assert_eq!(p.data, annexb(&[SLICE], &START_CODE));
        inspect_access_unit(VideoCodec::H264, &p.data, false).unwrap();
    }

    #[test]
    fn length_prefixed_output_is_converted() {
        let avcc = annexb::length_prefixed(&[SPS, PPS, IDR]);
        let unit = normalize(&avcc, &[]).unwrap();
        assert!(unit.is_keyframe);
        assert_eq!(unit.data, annexb(&[SPS, PPS, IDR], &START_CODE));
        assert_eq!(normalize(&[], &[]), None);
        assert_eq!(normalize(&[0, 0, 0, 9, 1], &[]), None, "a corrupt length");
    }
}
