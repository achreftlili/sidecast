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
        let unit = AccessUnit(annexB: annexB, codec: codec)
        let ps = unit.parameterSets
        if !ps.isEmpty, ps != parameterSets {
            parameterSets = ps
            rebuildSession()
        }
        guard let session, let format, !unit.slices.isEmpty else {
            setupErrors += 1
            return !ps.isEmpty
        }
        guard let sample = VideoFormat.sampleBuffer(lengthPrefixed: unit.lengthPrefixedSlices, format: format) else {
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

    private func rebuildSession() {
        if let session {
            VTDecompressionSessionWaitForAsynchronousFrames(session)
            VTDecompressionSessionInvalidate(session)
            self.session = nil
        }
        let fmt: CMVideoFormatDescription
        switch VideoFormat.formatDescription(codec: codec, parameterSets: parameterSets) {
        case .success(let f): fmt = f
        case .failure(let error):
            print("probe: cannot build format description (\(error.status))")
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
}
