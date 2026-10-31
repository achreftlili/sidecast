import AVFoundation
import CoreMedia
import SidecastKit
import UIKit
import VideoToolbox

/// What the decoder is doing, for the overlay.
enum DecoderStatus: Equatable {
    case idle
    case running(hasPicture: Bool)
    case unsupported(config: StreamConfig, detail: String)
}

/// HELLO `codecs` (PROTOCOL §5): what this device decodes in hardware.
enum DecoderCapabilities {
    /// Apple's A-series and M-series decoders handle 4K-class streams (HEVC Main,
    /// H.264 level 5.2) at 60 fps; iOS has no per-size query like Android's, so
    /// this is a fixed, conservative box that covers every iPad and iPhone panel.
    static let maxWidth = 4096
    static let maxHeight = 2304

    /// HEVC first when the hardware decodes it, then H.264 (every iOS device does).
    /// `override` (a testing aid) lists codec names to advertise instead.
    static func advertised(override: [String]? = nil) -> [CodecInfo] {
        let names: [String]
        if let override, !override.isEmpty {
            names = override
        } else {
            names = (VTIsHardwareDecodeSupported(kCMVideoCodecType_HEVC) ? [VideoCodec.hevc.rawValue] : [])
                + [VideoCodec.h264.rawValue]
        }
        return names.map { CodecInfo(name: $0, maxWidth: maxWidth, maxHeight: maxHeight) }
    }
}

/// The video layer: an AVSampleBufferDisplayLayer sized to the letterboxed picture.
final class VideoView: UIView {
    override class var layerClass: AnyClass { AVSampleBufferDisplayLayer.self }

    var displayLayer: AVSampleBufferDisplayLayer { layer as! AVSampleBufferDisplayLayer }

    override init(frame: CGRect) {
        super.init(frame: frame)
        backgroundColor = .black
        isUserInteractionEnabled = false
        // The view itself is sized to the picture's aspect ratio (Letterbox), so
        // touches and pixels use the same mapping.
        displayLayer.videoGravity = .resize
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
}

/// Decodes VIDEO_FRAMEs (Annex-B, PROTOCOL §6) with VideoToolbox and shows each
/// picture the moment it is decoded: no jitter buffer, no reordering, every
/// sample marked "display immediately". Asks for a keyframe after every
/// (re)configuration and whenever decoding fails, and drops frames until it arrives.
///
/// `configure` / `decode` / `reset` run on the session queue; the
/// VideoToolbox callbacks hop to the session queue or the render queue.
final class VideoDecoder {
    /// Status changes, on the main queue.
    var onStatus: ((DecoderStatus) -> Void)?

    private let renderer: AVSampleBufferVideoRenderer
    private let sessionQueue: DispatchQueue
    private let requestKeyframe: () -> Void
    private let renderQueue = DispatchQueue(label: "sidecast.render", qos: .userInteractive)

    // Session queue.
    private var codec: VideoCodec?
    private var config: StreamConfig?
    private var session: VTDecompressionSession?
    private var format: CMVideoFormatDescription?
    private var parameterSets: [Data] = []
    private var waitingForKeyframe = true
    private var lastKeyframeRequest = -TimeInterval.infinity
    private var status = DecoderStatus.idle
    private var hasPicture = false

    // Any thread.
    private let counterLock = NSLock()
    private var _rendered = 0
    private var _dropped = 0
    private var pictureFormat: CMVideoFormatDescription?

    /// - Parameter requestKeyframe: sends KEYFRAME_REQUEST; called on the session queue.
    init(renderer: AVSampleBufferVideoRenderer, sessionQueue: DispatchQueue, requestKeyframe: @escaping () -> Void) {
        self.renderer = renderer
        self.sessionQueue = sessionQueue
        self.requestKeyframe = requestKeyframe
    }

    /// Pictures handed to the display so far.
    var rendered: Int { counterLock.withLock { _rendered } }
    /// Frames not shown: decode errors and frames skipped while waiting for a keyframe.
    var dropped: Int { counterLock.withLock { _dropped } }

    /// STREAM_CONFIG: a new decoder for the codec and size, starting at the next keyframe.
    func configure(_ config: StreamConfig) {
        dispatchPrecondition(condition: .onQueue(sessionQueue))
        self.config = config
        invalidateSession()
        parameterSets = []
        guard let codec = VideoCodec(rawValue: config.codec.lowercased()) else {
            self.codec = nil
            return setStatus(.unsupported(config: config, detail: "unknown codec"))
        }
        self.codec = codec
        waitingForKeyframe = true
        setStatus(.running(hasPicture: hasPicture))
        // §6: after every decoder (re)configuration, including the first one.
        askForKeyframe(force: true)
    }

    func decode(_ frame: VideoFrame) {
        dispatchPrecondition(condition: .onQueue(sessionQueue))
        guard let codec, case .running = status else { return countDropped() }
        if waitingForKeyframe, !frame.isKeyframe {
            countDropped()
            return askForKeyframe(force: false)
        }
        let unit = AccessUnit(annexB: frame.data, codec: codec)
        if !unit.parameterSets.isEmpty, unit.parameterSets != parameterSets || session == nil {
            guard rebuildSession(codec: codec, parameterSets: unit.parameterSets) else { return }
        }
        guard let session, let format, !unit.slices.isEmpty,
              let sample = VideoFormat.sampleBuffer(lengthPrefixed: unit.lengthPrefixedSlices, format: format,
                                                    presentationTime: CMTime(value: CMTimeValue(frame.ptsMicros),
                                                                             timescale: 1_000_000)) else {
            countDropped()
            return needKeyframe()
        }
        waitingForKeyframe = false
        let result = VTDecompressionSessionDecodeFrame(
            session, sampleBuffer: sample, flags: [._EnableAsynchronousDecompression, ._1xRealTimePlayback],
            infoFlagsOut: nil) { [weak self] status, _, image, _, _ in
            self?.decoded(status: status, image: image)
        }
        if result != noErr {
            Log.video.error("decode failed: \(result)")
            countDropped()
            if result == kVTInvalidSessionErr {
                // The system tore the decoder down (e.g. in the background): rebuild at the next keyframe.
                invalidateSession()
                parameterSets = []
            }
            needKeyframe()
        }
    }

    /// The connection ended: forget the decoder, keep the last picture on screen.
    func reset() {
        dispatchPrecondition(condition: .onQueue(sessionQueue))
        invalidateSession()
        parameterSets = []
        codec = nil
        config = nil
        waitingForKeyframe = true
        hasPicture = false
        setStatus(.idle)
    }

    // MARK: - Private

    private func rebuildSession(codec: VideoCodec, parameterSets: [Data]) -> Bool {
        invalidateSession()
        switch VideoFormat.formatDescription(codec: codec, parameterSets: parameterSets) {
        case .failure(let error):
            Log.video.error("bad parameter sets (\(error.status))")
            countDropped()
            needKeyframe()
            return false
        case .success(let fmt):
            let attributes: [CFString: Any] = [kCVPixelBufferIOSurfacePropertiesKey: [CFString: Any]()]
            var created: VTDecompressionSession?
            let status = VTDecompressionSessionCreate(allocator: kCFAllocatorDefault, formatDescription: fmt,
                                                      decoderSpecification: nil,
                                                      imageBufferAttributes: attributes as CFDictionary,
                                                      outputCallback: nil, decompressionSessionOut: &created)
            guard status == noErr, let created else {
                Log.video.error("cannot create a \(codec.rawValue, privacy: .public) decoder: \(status)")
                if let config { setStatus(.unsupported(config: config, detail: "VideoToolbox error \(status)")) }
                return false
            }
            VTSessionSetProperty(created, key: kVTDecompressionPropertyKey_RealTime, value: kCFBooleanTrue)
            session = created
            format = fmt
            self.parameterSets = parameterSets
            let dims = CMVideoFormatDescriptionGetDimensions(fmt)
            Log.video.info("decoder ready: \(codec.rawValue, privacy: .public) \(dims.width)x\(dims.height)")
            return true
        }
    }

    private func invalidateSession() {
        if let session {
            VTDecompressionSessionInvalidate(session)
        }
        session = nil
        format = nil
    }

    /// VideoToolbox callback (its own thread).
    private func decoded(status: OSStatus, image: CVImageBuffer?) {
        guard status == noErr, let image else {
            sessionQueue.async { [weak self] in
                guard let self else { return }
                Log.video.error("frame did not decode: \(status)")
                self.countDropped()
                self.needKeyframe()
            }
            return
        }
        guard let sample = displaySample(image) else { return countDropped() }
        renderQueue.async { [renderer] in
            // A failed or interrupted renderer (e.g. after the app was in the background) must be flushed first.
            if renderer.status == .failed || renderer.requiresFlushToResumeDecoding { renderer.flush() }
            renderer.enqueue(sample)
        }
        counterLock.withLock { _rendered += 1 }
        sessionQueue.async { [weak self] in
            guard let self, !self.hasPicture, case .running = self.status else { return }
            self.hasPicture = true
            self.setStatus(.running(hasPicture: true))
        }
    }

    /// The decoded picture as a sample the display layer shows at once.
    private func displaySample(_ image: CVImageBuffer) -> CMSampleBuffer? {
        let format: CMVideoFormatDescription? = counterLock.withLock {
            if let f = pictureFormat, CMVideoFormatDescriptionMatchesImageBuffer(f, imageBuffer: image) { return f }
            var f: CMVideoFormatDescription?
            CMVideoFormatDescriptionCreateForImageBuffer(allocator: kCFAllocatorDefault, imageBuffer: image,
                                                         formatDescriptionOut: &f)
            pictureFormat = f
            return f
        }
        guard let format else { return nil }
        var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: CMClockGetTime(CMClockGetHostTimeClock()),
                                        decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReadyWithImageBuffer(allocator: kCFAllocatorDefault, imageBuffer: image,
                                                       formatDescription: format, sampleTiming: &timing,
                                                       sampleBufferOut: &sample) == noErr,
              let sample else { return nil }
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true),
           CFArrayGetCount(attachments) > 0 {
            let dict = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(dict, Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }
        return sample
    }

    private func needKeyframe() {
        waitingForKeyframe = true
        askForKeyframe(force: false)
    }

    /// At most one request per half second: the host answers within one frame,
    /// and a burst of errors must not turn into a burst of keyframes.
    private func askForKeyframe(force: Bool) {
        let now = ProcessInfo.processInfo.systemUptime
        guard force || now - lastKeyframeRequest >= 0.5 else { return }
        lastKeyframeRequest = now
        requestKeyframe()
    }

    private func countDropped() { counterLock.withLock { _dropped += 1 } }

    private func setStatus(_ new: DecoderStatus) {
        guard new != status else { return }
        status = new
        DispatchQueue.main.async { [weak self] in self?.onStatus?(new) }
    }
}
