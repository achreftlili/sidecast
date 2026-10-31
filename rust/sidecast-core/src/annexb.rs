//! Annex-B elementary streams (VIDEO_FRAME data): conversion from
//! length-prefixed NAL units and the structural checks a conformance client
//! applies to every access unit.

use std::fmt;

use crate::streaming::VideoCodec;

pub const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// Splits an Annex-B stream into NAL units (without start codes). Accepts
/// 3- and 4-byte start codes; bytes before the first start code are dropped.
pub fn split(stream: &[u8]) -> Vec<&[u8]> {
    let mut units = Vec::new();
    let mut nal_start: Option<usize> = None;
    let n = stream.len();
    let mut i = 0;
    while i + 2 < n {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            if let Some(s) = nal_start {
                let mut end = i;
                if end > s && stream[end - 1] == 0 {
                    end -= 1; // 4-byte start code
                }
                if end > s {
                    units.push(&stream[s..end]);
                }
            }
            i += 3;
            nal_start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(s) = nal_start
        && s < n
    {
        units.push(&stream[s..]);
    }
    units
}

/// One Annex-B access unit from length-prefixed NAL units (AVCC/HVCC, as
/// hardware encoders emit them), with `parameter_sets` (keyframes only) in
/// front. `None` when the length-prefixed data is corrupt.
pub fn from_length_prefixed(data: &[u8], nal_length_size: usize, parameter_sets: &[&[u8]]) -> Option<Vec<u8>> {
    if !(1..=4).contains(&nal_length_size) {
        return None;
    }
    let extra: usize = parameter_sets.iter().map(|p| p.len() + 4).sum();
    let mut out = Vec::with_capacity(data.len() + extra + 64);
    for ps in parameter_sets {
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(ps);
    }
    let mut offset = 0;
    while offset < data.len() {
        if data.len() - offset < nal_length_size {
            return None;
        }
        let length = data[offset..offset + nal_length_size].iter().fold(0usize, |acc, b| (acc << 8) | *b as usize);
        offset += nal_length_size;
        if length == 0 || data.len() - offset < length {
            return None;
        }
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&data[offset..offset + length]);
        offset += length;
    }
    Some(out)
}

/// Re-encodes NAL units with 4-byte big-endian length prefixes.
pub fn length_prefixed(units: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(units.iter().map(|u| u.len() + 4).sum());
    for u in units {
        out.extend_from_slice(&(u.len() as u32).to_be_bytes());
        out.extend_from_slice(u);
    }
    out
}

/// H.264 NAL unit type (`byte0 & 0x1F`).
pub fn h264_nal_type(unit: &[u8]) -> Option<u8> {
    unit.first().map(|b| b & 0x1F)
}

/// HEVC NAL unit type (`(byte0 >> 1) & 0x3F`).
pub fn hevc_nal_type(unit: &[u8]) -> Option<u8> {
    unit.first().map(|b| (b >> 1) & 0x3F)
}

/// Why an access unit is not acceptable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnexBError {
    Empty,
    NoStartCode,
    EmptyNalUnit,
    ForbiddenBitSet {
        nal_type: u8,
    },
    InvalidNalType {
        nal_type: u8,
    },
    /// HEVC `nuh_temporal_id_plus1` must not be 0.
    InvalidTemporalId,
    NoSlice,
    KeyframeWithoutParameterSets,
    KeyframeWithoutRandomAccessSlice,
}

impl fmt::Display for AnnexBError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnnexBError::Empty => f.write_str("empty access unit"),
            AnnexBError::NoStartCode => f.write_str("does not start with a 00 00 01 / 00 00 00 01 start code"),
            AnnexBError::EmptyNalUnit => f.write_str("empty NAL unit between start codes"),
            AnnexBError::ForbiddenBitSet { nal_type } => write!(f, "forbidden_zero_bit set (NAL type {nal_type})"),
            AnnexBError::InvalidNalType { nal_type } => write!(f, "reserved/unspecified NAL type {nal_type}"),
            AnnexBError::InvalidTemporalId => f.write_str("HEVC nuh_temporal_id_plus1 is 0"),
            AnnexBError::NoSlice => f.write_str("no coded slice in the access unit"),
            AnnexBError::KeyframeWithoutParameterSets => f.write_str("keyframe without parameter sets"),
            AnnexBError::KeyframeWithoutRandomAccessSlice => f.write_str("keyframe without an IDR/IRAP slice"),
        }
    }
}

impl std::error::Error for AnnexBError {}

/// What one access unit contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessUnitInfo {
    pub nal_types: Vec<u8>,
    /// SPS + PPS (and VPS for HEVC) are all present.
    pub has_parameter_sets: bool,
    /// An IDR slice (H.264) or IRAP slice (HEVC).
    pub has_random_access_slice: bool,
}

/// Checks the structure of one access unit of `codec`: start codes, NAL
/// headers, at least one slice, and for keyframes the parameter sets and a
/// random-access slice (PROTOCOL §6).
pub fn inspect_access_unit(codec: VideoCodec, data: &[u8], is_keyframe: bool) -> Result<AccessUnitInfo, AnnexBError> {
    if data.is_empty() {
        return Err(AnnexBError::Empty);
    }
    if !(data.starts_with(&[0, 0, 1]) || data.starts_with(&START_CODE)) {
        return Err(AnnexBError::NoStartCode);
    }
    let units = split(data);
    // Count start codes to catch empty NAL units (split skips them).
    let start_codes = data.windows(3).filter(|w| w == &[0, 0, 1]).count();
    if units.len() < start_codes {
        return Err(AnnexBError::EmptyNalUnit);
    }
    let mut info = AccessUnitInfo { nal_types: Vec::new(), has_parameter_sets: false, has_random_access_slice: false };
    let (mut vps, mut sps, mut pps, mut slice) = (false, false, false, false);
    for unit in units {
        if unit[0] & 0x80 != 0 {
            return Err(AnnexBError::ForbiddenBitSet { nal_type: unit[0] & 0x7F });
        }
        match codec {
            VideoCodec::H264 => {
                let t = unit[0] & 0x1F;
                match t {
                    1..=5 => {
                        slice = true;
                        info.has_random_access_slice |= t == 5;
                    }
                    7 => sps = true,
                    8 => pps = true,
                    6 | 9..=23 => {}
                    _ => return Err(AnnexBError::InvalidNalType { nal_type: t }),
                }
                info.nal_types.push(t);
            }
            VideoCodec::Hevc => {
                let t = (unit[0] >> 1) & 0x3F;
                if unit.len() < 2 {
                    return Err(AnnexBError::EmptyNalUnit);
                }
                if unit[1] & 0x07 == 0 {
                    return Err(AnnexBError::InvalidTemporalId);
                }
                match t {
                    0..=9 => slice = true,
                    16..=21 => {
                        slice = true;
                        info.has_random_access_slice = true;
                    }
                    32 => vps = true,
                    33 => sps = true,
                    34 => pps = true,
                    35..=40 | 10..=15 | 22..=31 | 41..=47 => {}
                    _ => return Err(AnnexBError::InvalidNalType { nal_type: t }),
                }
                info.nal_types.push(t);
            }
        }
    }
    info.has_parameter_sets = sps && pps && (codec == VideoCodec::H264 || vps);
    if !slice {
        return Err(AnnexBError::NoSlice);
    }
    if is_keyframe && !info.has_parameter_sets {
        return Err(AnnexBError::KeyframeWithoutParameterSets);
    }
    if is_keyframe && !info.has_random_access_slice {
        return Err(AnnexBError::KeyframeWithoutRandomAccessSlice);
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_prefixed_to_annexb_with_parameter_sets() {
        let avcc = [0, 0, 0, 2, 0x26, 0x01, 0, 0, 0, 3, 0x02, 0xAA, 0xBB];
        let (vps, sps, pps): (&[u8], &[u8], &[u8]) = (&[0x40, 0x01], &[0x42, 0x01], &[0x44, 0x01]);
        let out = from_length_prefixed(&avcc, 4, &[vps, sps, pps]).unwrap();
        assert_eq!(
            out,
            [
                0, 0, 0, 1, 0x40, 0x01, 0, 0, 0, 1, 0x42, 0x01, 0, 0, 0, 1, 0x44, 0x01, 0, 0, 0, 1, 0x26, 0x01, 0, 0,
                0, 1, 0x02, 0xAA, 0xBB
            ]
        );
    }

    #[test]
    fn corrupt_length_is_rejected() {
        assert_eq!(from_length_prefixed(&[0, 0, 0, 9, 1, 2], 4, &[]), None);
        assert_eq!(from_length_prefixed(&[0, 0], 4, &[]), None);
        assert_eq!(from_length_prefixed(&[], 4, &[]), Some(Vec::new()));
    }

    #[test]
    fn split_handles_three_and_four_byte_start_codes() {
        let stream = [0, 0, 0, 1, 0x40, 0x01, 0, 0, 1, 0x42, 0x01, 0, 0, 0, 1, 0x26, 0xFF];
        let units = split(&stream);
        assert_eq!(units, [&[0x40, 0x01][..], &[0x42, 0x01], &[0x26, 0xFF]]);
        assert_eq!(units.iter().filter_map(|u| hevc_nal_type(u)).collect::<Vec<_>>(), [32, 33, 19]);
        assert_eq!(h264_nal_type(&[0x67]), Some(7));
    }

    #[test]
    fn round_trip() {
        let units: [&[u8]; 2] = [&[0x40, 1, 2], &[0x26, 9, 9, 9]];
        let lp = length_prefixed(&units);
        let annexb = from_length_prefixed(&lp, 4, &[]).unwrap();
        assert_eq!(split(&annexb), units);
    }

    #[test]
    fn inspects_h264_access_units() {
        let key = [0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65, 3];
        let info = inspect_access_unit(VideoCodec::H264, &key, true).unwrap();
        assert_eq!(info.nal_types, [7, 8, 5]);
        assert!(info.has_parameter_sets && info.has_random_access_slice);
        let delta = [0, 0, 0, 1, 0x41, 9];
        assert!(inspect_access_unit(VideoCodec::H264, &delta, false).is_ok());
        assert_eq!(inspect_access_unit(VideoCodec::H264, &delta, true), Err(AnnexBError::KeyframeWithoutParameterSets));
        assert_eq!(inspect_access_unit(VideoCodec::H264, &[0x41, 9], false), Err(AnnexBError::NoStartCode));
        assert_eq!(inspect_access_unit(VideoCodec::H264, &[0, 0, 0, 1, 0x67, 1], false), Err(AnnexBError::NoSlice));
        assert_eq!(
            inspect_access_unit(VideoCodec::H264, &[0, 0, 0, 1, 0xC1, 1], false),
            Err(AnnexBError::ForbiddenBitSet { nal_type: 0x41 })
        );
        assert_eq!(
            inspect_access_unit(VideoCodec::H264, &[0, 0, 0, 1, 0, 0, 0, 1, 0x41, 1], false),
            Err(AnnexBError::EmptyNalUnit)
        );
    }

    #[test]
    fn inspects_hevc_access_units() {
        // VPS, SPS, PPS, IDR_W_RADL (19) with temporal id 1.
        let key = [0, 0, 0, 1, 0x40, 1, 0, 0, 0, 1, 0x42, 1, 0, 0, 0, 1, 0x44, 1, 0, 0, 0, 1, 0x26, 1, 0xAA];
        let info = inspect_access_unit(VideoCodec::Hevc, &key, true).unwrap();
        assert_eq!(info.nal_types, [32, 33, 34, 19]);
        assert!(inspect_access_unit(VideoCodec::Hevc, &[0, 0, 0, 1, 0x02, 1, 7], false).is_ok());
        assert_eq!(
            inspect_access_unit(VideoCodec::Hevc, &[0, 0, 0, 1, 0x02, 0, 7], false),
            Err(AnnexBError::InvalidTemporalId)
        );
        // Without the VPS a HEVC keyframe is incomplete.
        let no_vps = [0, 0, 0, 1, 0x42, 1, 0, 0, 0, 1, 0x44, 1, 0, 0, 0, 1, 0x26, 1, 0xAA];
        assert_eq!(
            inspect_access_unit(VideoCodec::Hevc, &no_vps, true),
            Err(AnnexBError::KeyframeWithoutParameterSets)
        );
    }
}
