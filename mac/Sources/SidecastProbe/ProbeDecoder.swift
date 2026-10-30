import CoreMedia
import Foundation
import SidecastKit
import VideoToolbox

/// Decodes the Annex-B stream with VideoToolbox to prove every frame is valid.
final class ProbeDecoder {
    let codec: VideoCodec
    let width: Int
    let height: Int

    private var session: VTDecompressionSession?
    private var format: CMVideoFormatDescription?
    private var parameterSets: [Data] = []
    private let lock = NSLock()
    private var _decoded = 0
    private var _errors = 0
    private var _sizeMismatches = 0
    private(set) var submitted = 0
    private(set) var setupErrors = 0

    init(codec: VideoCodec, width: Int, height: Int) {
        self.codec = codec
        self.width = width
        self.height = height
    }

    var decoded: Int { lock.withLock { _decoded } }
    var errors: Int { lock.withLock { _errors + setupErrors } }
    var sizeMismatches: Int { lock.withLock { _sizeMismatches } }

    /// Feeds one access unit. Returns whether it carried parameter sets.
    @discardableResult
    func decode(_ annexB: Data) -> Bool {
        let units = AnnexB.split(annexB)
        var ps: [Data] = []
        var slices: [Data] = []
        for unit in units {
            if isParameterSet(unit) { ps.append(unit) } else if !isIgnorable(unit) { slices.append(unit) }
        }
        if !ps.isEmpty, ps != parameterSets {
            parameterSets = ps
            rebuildSession()
        }
        guard let session, let format, !slices.isEmpty else {
            setupErrors += 1
            return !ps.isEmpty
        }
        let payload = AnnexB.lengthPrefixed(slices)
        guard let sample = makeSample(payload, format: format) else {
            setupErrors += 1
            return !ps.isEmpty
        }
        submitted += 1
        let expected = (width, height)
        let status = VTDecompressionSessionDecodeFrame(session, sampleBuffer: sample,
                                                       flags: [._EnableAsynchronousDecompression],
                                                       infoFlagsOut: nil) { [weak self] status, _, image, _, _ in
            guard let self else { return }
            self.lock.withLock {
                if status == noErr, let image {
                    self._decoded += 1
                    if CVPixelBufferGetWidth(image) != expected.0 || CVPixelBufferGetHeight(image) != expected.1 {
                        self._sizeMismatches += 1
                    }
                } else {
                    self._errors += 1
                }
            }
        }
        if status != noErr { lock.withLock { _errors += 1 } }
        return !ps.isEmpty
    }

    func finish() {
        guard let session else { return }
        VTDecompressionSessionWaitForAsynchronousFrames(session)
        VTDecompressionSessionInvalidate(session)
        self.session = nil
    }

    // MARK: - Private

    private func isParameterSet(_ unit: Data) -> Bool {
        switch codec {
        case .hevc: return (32...34).contains(AnnexB.hevcNALType(unit) ?? 0)
        case .h264: return [7, 8].contains(AnnexB.h264NALType(unit) ?? 0)
        }
    }

    /// Access unit delimiters and SEI carry no picture data.
    private func isIgnorable(_ unit: Data) -> Bool {
        switch codec {
        case .hevc: return [35, 39, 40].contains(AnnexB.hevcNALType(unit) ?? 0)
        case .h264: return [6, 9].contains(AnnexB.h264NALType(unit) ?? 0)
        }
    }

    private func rebuildSession() {
        if let session {
            VTDecompressionSessionWaitForAsynchronousFrames(session)
            VTDecompressionSessionInvalidate(session)
            self.session = nil
        }
        var fmt: CMFormatDescription?
        let status = withParameterSetPointers(parameterSets) { pointers, sizes -> OSStatus in
            switch codec {
            case .hevc:
                return CMVideoFormatDescriptionCreateFromHEVCParameterSets(
                    allocator: kCFAllocatorDefault, parameterSetCount: pointers.count, parameterSetPointers: pointers,
                    parameterSetSizes: sizes, nalUnitHeaderLength: 4, extensions: nil, formatDescriptionOut: &fmt)
            case .h264:
                return CMVideoFormatDescriptionCreateFromH264ParameterSets(
                    allocator: kCFAllocatorDefault, parameterSetCount: pointers.count, parameterSetPointers: pointers,
                    parameterSetSizes: sizes, nalUnitHeaderLength: 4, formatDescriptionOut: &fmt)
            }
        }
        guard status == noErr, let fmt else {
            print("probe: cannot build format description (\(status))")
            setupErrors += 1
            return
        }
        format = fmt
        let attrs: [CFString: Any] = [kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange]
        var created: VTDecompressionSession?
        let s = VTDecompressionSessionCreate(allocator: kCFAllocatorDefault, formatDescription: fmt,
                                             decoderSpecification: nil, imageBufferAttributes: attrs as CFDictionary,
                                             outputCallback: nil, decompressionSessionOut: &created)
        guard s == noErr, let created else {
            print("probe: cannot create decoder (\(s))")
            setupErrors += 1
            return
        }
        session = created
    }

    private func makeSample(_ payload: Data, format: CMFormatDescription) -> CMSampleBuffer? {
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
        guard CMSampleBufferCreateReady(allocator: kCFAllocatorDefault, dataBuffer: block, formatDescription: format,
                                        sampleCount: 1, sampleTimingEntryCount: 0, sampleTimingArray: nil,
                                        sampleSizeEntryCount: 1, sampleSizeArray: &size,
                                        sampleBufferOut: &sample) == noErr else { return nil }
        return sample
    }

    private func withParameterSetPointers<R>(_ sets: [Data],
                                             _ body: ([UnsafePointer<UInt8>], [Int]) -> R) -> R {
        let buffers = sets.map { Array($0) }
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
