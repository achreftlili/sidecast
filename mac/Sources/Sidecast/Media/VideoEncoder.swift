import CoreMedia
import Foundation
import SidecastKit
import VideoToolbox

/// One encoded access unit, already in Annex-B form.
struct EncodedFrame {
    let data: Data
    let isKeyframe: Bool
    let ptsMicros: UInt64
}

enum VideoEncoderError: Error, CustomStringConvertible {
    case sessionCreation(OSStatus)

    var description: String {
        switch self {
        case .sessionCreation(let s): return "VTCompressionSessionCreate failed (\(s))"
        }
    }
}

/// Low-latency VideoToolbox encoder (settings ported from SideScreen, minus
/// its 60 Mbps bitrate floor): real-time, no frame reordering, zero frame
/// delay, a keyframe at least every 2 s, live bitrate changes. Output is
/// Annex-B with parameter sets on keyframes. Call it from one serial queue.
final class VideoEncoder {
    let codec: VideoCodec
    let width: Int
    let height: Int
    let fps: Int
    private(set) var bitrate: Int
    private var session: VTCompressionSession?
    private var lastEncodeMicros: UInt64 = 0

    init(codec: VideoCodec, width: Int, height: Int, fps: Int, bitrate: Int) throws {
        self.codec = codec
        self.width = width
        self.height = height
        self.fps = fps
        self.bitrate = bitrate

        var created: VTCompressionSession?
        let spec: [CFString: Any] = [kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: true]
        let status = VTCompressionSessionCreate(
            allocator: kCFAllocatorDefault,
            width: Int32(width),
            height: Int32(height),
            codecType: codec == .hevc ? kCMVideoCodecType_HEVC : kCMVideoCodecType_H264,
            encoderSpecification: spec as CFDictionary,
            imageBufferAttributes: nil,
            compressedDataAllocator: nil,
            outputCallback: nil,
            refcon: nil,
            compressionSessionOut: &created)
        guard status == noErr, let session = created else { throw VideoEncoderError.sessionCreation(status) }
        self.session = session
        configure(session)
        VTCompressionSessionPrepareToEncodeFrames(session)
    }

    deinit { invalidate() }

    private func configure(_ s: VTCompressionSession) {
        set(s, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue)
        // H.264 Main rather than High: some low-end vendor decoders reject
        // High's 8×8 transform (SideScreen finding); H.264 clients are the low end.
        set(s, kVTCompressionPropertyKey_ProfileLevel,
            codec == .hevc ? kVTProfileLevel_HEVC_Main_AutoLevel : kVTProfileLevel_H264_Main_AutoLevel)
        set(s, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse)
        set(s, kVTCompressionPropertyKey_MaxFrameDelayCount, 0 as CFNumber)
        set(s, kVTCompressionPropertyKey_ExpectedFrameRate, fps as CFNumber)
        set(s, kVTCompressionPropertyKey_MaxKeyFrameInterval, (fps * 2) as CFNumber)
        set(s, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, SidecastProtocol.keyframeInterval as CFNumber)
        set(s, kVTCompressionPropertyKey_PrioritizeEncodingSpeedOverQuality, kCFBooleanTrue)
        // Explicit BT.709 so every Android decoder picks the same YUV→RGB matrix (SideScreen #55).
        set(s, kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2)
        set(s, kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2)
        set(s, kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2)
        applyBitrate(s)
    }

    private func applyBitrate(_ s: VTCompressionSession) {
        set(s, kVTCompressionPropertyKey_AverageBitRate, bitrate as CFNumber)
        // Hard ceiling: 1.5× the average over any 1 s window, so keyframes
        // cannot burst far past what the link was measured to carry.
        let bytesPerSecond = Double(bitrate) / 8 * 1.5
        set(s, kVTCompressionPropertyKey_DataRateLimits, [bytesPerSecond, 1.0] as CFArray)
    }

    private func set(_ s: VTCompressionSession, _ key: CFString, _ value: CFTypeRef) {
        let status = VTSessionSetProperty(s, key: key, value: value)
        if status != noErr {
            Log.debug("encoder: \(key) not supported (\(status))")
        }
    }

    /// Changes the target bitrate of the running session.
    func setBitrate(_ bps: Int) {
        guard bps != bitrate, let session else { return }
        bitrate = bps
        applyBitrate(session)
    }

    /// Encodes one frame. `completion` runs exactly once, on a VideoToolbox
    /// thread, with nil when the encoder dropped the frame or failed.
    func encode(_ pixelBuffer: CVPixelBuffer, ptsMicros: UInt64, forceKeyframe: Bool,
                completion: @escaping (EncodedFrame?) -> Void) {
        guard let session else { return completion(nil) }
        // VideoToolbox wants increasing timestamps; a re-sent cached frame may
        // carry a later time than the next fresh capture. The wire keeps ptsMicros.
        let encodeMicros = max(ptsMicros, lastEncodeMicros &+ 1)
        lastEncodeMicros = encodeMicros
        let pts = CMTime(value: CMTimeValue(encodeMicros), timescale: 1_000_000)
        let duration = CMTime(value: 1, timescale: CMTimeScale(fps))
        let props: CFDictionary? = forceKeyframe
            ? [kVTEncodeFrameOptionKey_ForceKeyFrame: kCFBooleanTrue] as CFDictionary : nil
        let codec = self.codec
        let once = OnceFlag()
        let status = VTCompressionSessionEncodeFrame(
            session, imageBuffer: pixelBuffer, presentationTimeStamp: pts, duration: duration,
            frameProperties: props, infoFlagsOut: nil
        ) { status, flags, sampleBuffer in
            guard once.claim() else { return }
            guard status == noErr, !flags.contains(.frameDropped), let sampleBuffer,
                  let frame = Self.annexB(from: sampleBuffer, codec: codec, ptsMicros: ptsMicros) else {
                if status != noErr { Log.debug("encoder: frame failed (\(status))") }
                return completion(nil)
            }
            completion(frame)
        }
        if status != noErr, once.claim() {
            Log.debug("encoder: VTCompressionSessionEncodeFrame failed (\(status))")
            completion(nil)
        }
    }

    func invalidate() {
        guard let session else { return }
        self.session = nil
        VTCompressionSessionCompleteFrames(session, untilPresentationTimeStamp: .invalid)
        VTCompressionSessionInvalidate(session)
    }

    // MARK: - AVCC/HVCC → Annex-B

    private static func annexB(from sample: CMSampleBuffer, codec: VideoCodec, ptsMicros: UInt64) -> EncodedFrame? {
        guard let block = CMSampleBufferGetDataBuffer(sample) else { return nil }
        let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: false) as? [[CFString: Any]]
        let isKeyframe = !(attachments?.first?[kCMSampleAttachmentKey_NotSync] as? Bool ?? false)

        var parameterSets: [Data] = []
        var nalLengthSize: Int32 = 4
        if let format = CMSampleBufferGetFormatDescription(sample) {
            parameterSets = Self.parameterSets(format, codec: codec, nalLengthSize: &nalLengthSize)
        }
        // A contiguous view of the block buffer (copied only if fragmented).
        var length = 0
        var pointer: UnsafeMutablePointer<CChar>?
        var contiguous: CMBlockBuffer = block
        if !CMBlockBufferIsRangeContiguous(block, atOffset: 0, length: 0) {
            var copy: CMBlockBuffer?
            guard CMBlockBufferCreateContiguous(allocator: nil, sourceBuffer: block, blockAllocator: nil,
                                                customBlockSource: nil, offsetToData: 0, dataLength: 0,
                                                flags: 0, blockBufferOut: &copy) == noErr, let copy else { return nil }
            contiguous = copy
        }
        guard CMBlockBufferGetDataPointer(contiguous, atOffset: 0, lengthAtOffsetOut: nil,
                                          totalLengthOut: &length, dataPointerOut: &pointer) == noErr,
              let pointer else { return nil }
        let raw = UnsafeRawBufferPointer(start: pointer, count: length)
        guard let data = AnnexB.fromLengthPrefixed(raw, nalLengthSize: Int(nalLengthSize),
                                                   parameterSets: isKeyframe ? parameterSets : []) else {
            return nil
        }
        return EncodedFrame(data: data, isKeyframe: isKeyframe, ptsMicros: ptsMicros)
    }

    private static func parameterSets(_ format: CMFormatDescription, codec: VideoCodec,
                                      nalLengthSize: inout Int32) -> [Data] {
        var count = 0
        let status: OSStatus = codec == .hevc
            ? CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(format, parameterSetIndex: 0, parameterSetPointerOut: nil,
                                                                parameterSetSizeOut: nil, parameterSetCountOut: &count,
                                                                nalUnitHeaderLengthOut: &nalLengthSize)
            : CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: 0, parameterSetPointerOut: nil,
                                                                parameterSetSizeOut: nil, parameterSetCountOut: &count,
                                                                nalUnitHeaderLengthOut: &nalLengthSize)
        guard status == noErr else { return [] }
        return (0..<count).compactMap { i in
            var ptr: UnsafePointer<UInt8>?
            var size = 0
            let s: OSStatus = codec == .hevc
                ? CMVideoFormatDescriptionGetHEVCParameterSetAtIndex(format, parameterSetIndex: i, parameterSetPointerOut: &ptr,
                                                                    parameterSetSizeOut: &size, parameterSetCountOut: nil,
                                                                    nalUnitHeaderLengthOut: nil)
                : CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: i, parameterSetPointerOut: &ptr,
                                                                    parameterSetSizeOut: &size, parameterSetCountOut: nil,
                                                                    nalUnitHeaderLengthOut: nil)
            guard s == noErr, let ptr else { return nil }
            return Data(bytes: ptr, count: size)
        }
    }
}
