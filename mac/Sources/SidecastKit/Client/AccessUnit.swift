import CoreMedia
import Foundation

/// One VIDEO_FRAME's Annex-B access unit, split for VideoToolbox: parameter
/// sets (which (re)build the format description) and the NAL units to decode.
public struct AccessUnit: Equatable, Sendable {
    /// HEVC VPS/SPS/PPS or H.264 SPS/PPS, in stream order.
    public var parameterSets: [Data]
    /// NAL units carrying picture data (delimiters and SEI dropped).
    public var slices: [Data]

    public init(parameterSets: [Data], slices: [Data]) {
        self.parameterSets = parameterSets
        self.slices = slices
    }

    public init(annexB: Data, codec: VideoCodec) {
        var ps: [Data] = []
        var slices: [Data] = []
        for unit in AnnexB.split(annexB) {
            if Self.isParameterSet(unit, codec: codec) {
                ps.append(unit)
            } else if !Self.isIgnorable(unit, codec: codec) {
                slices.append(unit)
            }
        }
        self.init(parameterSets: ps, slices: slices)
    }

    /// The slices with 4-byte big-endian length prefixes, as a CMSampleBuffer carries them.
    public var lengthPrefixedSlices: Data { AnnexB.lengthPrefixed(slices) }

    public static func isParameterSet(_ unit: Data, codec: VideoCodec) -> Bool {
        switch codec {
        case .hevc: return (32...34).contains(AnnexB.hevcNALType(unit) ?? 0)
        case .h264: return [7, 8].contains(AnnexB.h264NALType(unit) ?? 0)
        }
    }

    /// Access unit delimiters, SEI and end-of-sequence carry no picture data.
    public static func isIgnorable(_ unit: Data, codec: VideoCodec) -> Bool {
        switch codec {
        case .hevc: return [35, 36, 37, 39, 40].contains(AnnexB.hevcNALType(unit) ?? 0)
        case .h264: return [6, 9, 10, 11].contains(AnnexB.h264NALType(unit) ?? 0)
        }
    }
}

/// A CoreMedia call failed with `status`.
public struct VideoFormatError: Error, Equatable, Sendable {
    public var status: OSStatus
}

/// CoreMedia plumbing shared by the iOS decoder and `sidecast-probe`.
public enum VideoFormat {
    /// Format description for `codec` from its parameter sets (4-byte NAL length headers).
    public static func formatDescription(codec: VideoCodec,
                                         parameterSets: [Data]) -> Result<CMVideoFormatDescription, VideoFormatError> {
        guard !parameterSets.isEmpty, !parameterSets.contains(where: \.isEmpty) else {
            return .failure(VideoFormatError(status: kCMFormatDescriptionError_InvalidParameter))
        }
        var format: CMFormatDescription?
        let buffers = parameterSets.map { [UInt8]($0) }
        let status = withPointers(buffers) { pointers, sizes -> OSStatus in
            switch codec {
            case .hevc:
                return CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    allocator: kCFAllocatorDefault, parameterSetCount: pointers.count, parameterSetPointers: pointers,
                    parameterSetSizes: sizes, nalUnitHeaderLength: 4, extensions: nil, formatDescriptionOut: &format)
            case .h264:
                return CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: kCFAllocatorDefault, parameterSetCount: pointers.count, parameterSetPointers: pointers,
                    parameterSetSizes: sizes, nalUnitHeaderLength: 4, formatDescriptionOut: &format)
            }
        }
        guard status == noErr, let format else { return .failure(VideoFormatError(status: status == noErr ? -1 : status)) }
        return .success(format)
    }

    /// One compressed sample (length-prefixed NAL units) ready to decode.
    public static func sampleBuffer(lengthPrefixed payload: Data, format: CMFormatDescription,
                                    presentationTime: CMTime = .invalid) -> CMSampleBuffer? {
        guard !payload.isEmpty else { return nil }
        var block: CMBlockBuffer?
        guard CMBlockBufferCreateWithMemoryBlock(allocator: kCFAllocatorDefault, memoryBlock: nil,
                                                 blockLength: payload.count, blockAllocator: kCFAllocatorDefault,
                                                 customBlockSource: nil, offsetToData: 0, dataLength: payload.count,
                                                 flags: 0, blockBufferOut: &block) == noErr, let block else { return nil }
        let copied = payload.withUnsafeBytes { raw in
            CMBlockBufferReplaceDataBytes(with: raw.baseAddress!, blockBuffer: block, offsetIntoDestination: 0,
                                          dataLength: payload.count)
        }
        guard copied == noErr else { return nil }
        var sample: CMSampleBuffer?
        var size = payload.count
        var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: presentationTime,
                                        decodeTimeStamp: .invalid)
        let status = withUnsafePointer(to: &timing) { timingPointer in
            CMSampleBufferCreateReady(allocator: kCFAllocatorDefault, dataBuffer: block, formatDescription: format,
                                      sampleCount: 1, sampleTimingEntryCount: presentationTime.isValid ? 1 : 0,
                                      sampleTimingArray: presentationTime.isValid ? timingPointer : nil,
                                      sampleSizeEntryCount: 1, sampleSizeArray: &size, sampleBufferOut: &sample)
        }
        return status == noErr ? sample : nil
    }

    private static func withPointers<R>(_ buffers: [[UInt8]], _ body: ([UnsafePointer<UInt8>], [Int]) -> R) -> R {
        var pointers: [UnsafePointer<UInt8>] = []
        func recurse(_ i: Int) -> R {
            if i == buffers.count { return body(pointers, buffers.map(\.count)) }
            return buffers[i].withUnsafeBufferPointer { buf in
                pointers.append(buf.baseAddress!)
                return recurse(i + 1)
            }
        }
        return recurse(0)
    }
}
