import CoreMedia
import Foundation
import VideoToolbox
import XCTest
@testable import SidecastKit

// MARK: - Microphone fakes

private final class FakeRecorder: MicRecorder {
    var startResult = true
    var onSamples: (([Int16]) -> Void)?
    var onFailure: (() -> Void)?
    private(set) var stopped = false

    func start(onSamples: @escaping ([Int16]) -> Void, onFailure: @escaping () -> Void) -> Bool {
        self.onSamples = onSamples
        self.onFailure = onFailure
        return startResult
    }

    func stop() { stopped = true }
}

private final class FakePermission: MicPermission {
    var isGranted = false
    var isDenied = false
    var pending: ((Bool) -> Void)?
    var requests = 0

    func request(_ completion: @escaping (Bool) -> Void) {
        requests += 1
        pending = completion
    }

    func answer(_ granted: Bool) {
        isGranted = granted
        isDenied = !granted
        let p = pending
        pending = nil
        p?(granted)
    }
}

private final class FakeMicSink: MicSink {
    var states: [MicState] = []
    var packets: [AudioPacket] = []
    func sendMicState(_ state: MicState) { states.append(state) }
    func sendAudio(_ packet: AudioPacket) { packets.append(packet) }
}

/// The microphone state machine (PROTOCOL §7.2).
final class MicStreamerTests: XCTestCase {
    private var sink: FakeMicSink!
    private var permission: FakePermission!
    private var recorders: [FakeRecorder] = []
    private var enabled = true
    private var streamer: MicStreamer!

    override func setUp() {
        sink = FakeMicSink()
        permission = FakePermission()
        recorders = []
        enabled = true
        streamer = MicStreamer(sink: sink, makeRecorder: { [unowned self] in
            let r = FakeRecorder()
            self.recorders.append(r)
            return r
        }, permission: permission, isEnabled: { [unowned self] in self.enabled })
        streamer.enterForeground()
    }

    func testChunker() {
        var c = PCMChunker(size: 4)
        XCTAssertEqual(c.append([1, 2, 3]), [])
        XCTAssertEqual(c.append([4, 5, 6, 7, 8, 9]), [[1, 2, 3, 4], [5, 6, 7, 8]])
        c.reset()
        XCTAssertEqual(c.append([10, 11, 12, 13]), [[10, 11, 12, 13]])
    }

    func testCapturesInTenMillisecondPacketsAfterPermissionAndStopsOnRequest() {
        streamer.handle(.start)
        XCTAssertEqual(permission.requests, 1)
        XCTAssertTrue(recorders.isEmpty)
        permission.answer(true)
        XCTAssertEqual(recorders.count, 1)
        recorders[0].onSamples?([Int16](repeating: 7, count: 1000))
        XCTAssertEqual(sink.packets.map(\.seq), [0, 1])
        XCTAssertEqual(sink.packets.map(\.samples.count), [480, 480])
        XCTAssertEqual(sink.states, [MicState(capturing: true)])   // after the first AUDIO
        XCTAssertTrue(streamer.isCapturing)

        streamer.handle(.stop)
        XCTAssertTrue(recorders[0].stopped)
        XCTAssertEqual(sink.states.last, MicState(capturing: false, reason: .stopped))
        XCTAssertFalse(streamer.isCapturing)
        recorders[0].onSamples?([Int16](repeating: 1, count: 480))   // late callback: ignored
        XCTAssertEqual(sink.packets.count, 2)

        // A fresh request restarts seq at 0.
        streamer.handle(.start)
        recorders[1].onSamples?([Int16](repeating: 1, count: 480))
        XCTAssertEqual(sink.packets.last?.seq, 0)
    }

    func testRefusalsAreReported() {
        enabled = false
        streamer.handle(.start)
        XCTAssertEqual(sink.states, [MicState(capturing: false, reason: .disabled)])
        enabled = true
        streamer.settingChanged()
        permission.answer(false)
        XCTAssertEqual(sink.states.last, MicState(capturing: false, reason: .permissionDenied))
        XCTAssertTrue(recorders.isEmpty)
    }

    func testBackgroundStopsTheCaptureAndForegroundResumesIt() {
        permission.isGranted = true
        streamer.handle(.start)
        recorders[0].onSamples?([Int16](repeating: 1, count: 480))
        streamer.enterBackground()
        XCTAssertTrue(recorders[0].stopped)
        XCTAssertEqual(sink.states.last, MicState(capturing: false, reason: .background))
        streamer.enterForeground()
        XCTAssertEqual(recorders.count, 2)
        recorders[1].onSamples?([Int16](repeating: 1, count: 480))
        XCTAssertEqual(sink.states.last, MicState(capturing: true))
    }

    func testUnavailableMicrophone() {
        permission.isGranted = true
        streamer.handle(.start)
        recorders[0].onFailure?()
        XCTAssertEqual(sink.states.last, MicState(capturing: false, reason: .unavailable))
        XCTAssertTrue(recorders[0].stopped)

        setUp()
        permission.isGranted = true
        streamer.handle(MicControl(active: true))
        XCTAssertEqual(recorders.count, 1)
        streamer.sessionEnded()
        XCTAssertTrue(recorders[0].stopped)
        streamer.handle(.stop)   // nothing was wanted any more: no report
        XCTAssertEqual(sink.states, [])
    }

    func testFailedStartIsUnavailable() {
        permission.isGranted = true
        let streamer = MicStreamer(sink: sink, makeRecorder: {
            let r = FakeRecorder()
            r.startResult = false
            return r
        }, permission: permission, isEnabled: { true })
        streamer.enterForeground()
        streamer.handle(.start)
        XCTAssertEqual(sink.states, [MicState(capturing: false, reason: .unavailable)])
    }
}

/// Annex-B access units → VideoToolbox, as the iOS client and the probe decode them.
final class AccessUnitTests: XCTestCase {
    func testNALUnitsAreClassified() {
        // HEVC: VPS(32) SPS(33) PPS(34) AUD(35) SEI(39) IDR slice(19).
        let hevc = Data([0, 0, 0, 1, 0x40, 1, 0, 0, 0, 1, 0x42, 1, 0, 0, 1, 0x44, 1, 0, 0, 0, 1, 0x46, 1,
                         0, 0, 0, 1, 0x4E, 1, 0, 0, 0, 1, 0x26, 1, 0xAA])
        let unit = AccessUnit(annexB: hevc, codec: .hevc)
        XCTAssertEqual(unit.parameterSets, [Data([0x40, 1]), Data([0x42, 1]), Data([0x44, 1])])
        XCTAssertEqual(unit.slices, [Data([0x26, 1, 0xAA])])
        XCTAssertEqual(unit.lengthPrefixedSlices, Data([0, 0, 0, 3, 0x26, 1, 0xAA]))
        // H.264: SPS(7) PPS(8) AUD(9) IDR(5) non-IDR(1).
        let h264 = AccessUnit(annexB: Data([0, 0, 0, 1, 0x67, 0, 0, 0, 1, 0x68, 0, 0, 0, 1, 0x09, 0, 0, 0, 1, 0x65, 9,
                                            0, 0, 0, 1, 0x41, 8]), codec: .h264)
        XCTAssertEqual(h264.parameterSets, [Data([0x67]), Data([0x68])])
        XCTAssertEqual(h264.slices, [Data([0x65, 9]), Data([0x41, 8])])
        if case .success = VideoFormat.formatDescription(codec: .h264, parameterSets: []) { XCTFail("empty sets") }
    }

    /// Encodes one real frame with VideoToolbox, turns it into the Annex-B that
    /// VIDEO_FRAME carries, then rebuilds and decodes it the way the client does.
    func testEncodedFrameRoundTripsThroughTheClientPath() throws {
        let (annexB, width, height) = try encodeOneH264Frame()
        let unit = AccessUnit(annexB: annexB, codec: .h264)
        XCTAssertEqual(unit.parameterSets.count, 2)
        XCTAssertFalse(unit.slices.isEmpty)
        let format = try VideoFormat.formatDescription(codec: .h264, parameterSets: unit.parameterSets).get()
        let dims = CMVideoFormatDescriptionGetDimensions(format)
        XCTAssertEqual(Int(dims.width), width)
        XCTAssertEqual(Int(dims.height), height)
        let sample = try XCTUnwrap(VideoFormat.sampleBuffer(lengthPrefixed: unit.lengthPrefixedSlices, format: format,
                                                            presentationTime: CMTime(value: 1, timescale: 60)))
        var session: VTDecompressionSession?
        XCTAssertEqual(VTDecompressionSessionCreate(allocator: nil, formatDescription: format, decoderSpecification: nil,
                                                    imageBufferAttributes: nil, outputCallback: nil,
                                                    decompressionSessionOut: &session), noErr)
        let decoder = try XCTUnwrap(session)
        defer { VTDecompressionSessionInvalidate(decoder) }
        var decoded: CVImageBuffer?
        var callbackStatus: OSStatus = -1
        let status = VTDecompressionSessionDecodeFrame(decoder, sampleBuffer: sample, flags: [], infoFlagsOut: nil) {
            s, _, image, _, _ in
            callbackStatus = s
            decoded = image
        }
        VTDecompressionSessionWaitForAsynchronousFrames(decoder)
        XCTAssertEqual(status, noErr)
        XCTAssertEqual(callbackStatus, noErr)
        XCTAssertEqual(decoded.map(CVPixelBufferGetWidth), width)
    }

    private func encodeOneH264Frame() throws -> (Data, Int, Int) {
        let width = 128, height = 72
        var session: VTCompressionSession?
        let created = VTCompressionSessionCreate(allocator: nil, width: Int32(width), height: Int32(height),
                                                 codecType: kCMVideoCodecType_H264, encoderSpecification: nil,
                                                 imageBufferAttributes: nil, compressedDataAllocator: nil,
                                                 outputCallback: nil, refcon: nil, compressionSessionOut: &session)
        guard created == noErr, let encoder = session else { throw XCTSkip("no H.264 encoder here (\(created))") }
        defer { VTCompressionSessionInvalidate(encoder) }
        var pixelBuffer: CVPixelBuffer?
        CVPixelBufferCreate(nil, width, height, kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, nil, &pixelBuffer)
        let image = try XCTUnwrap(pixelBuffer)
        var output: CMSampleBuffer?
        let status = VTCompressionSessionEncodeFrame(encoder, imageBuffer: image, presentationTimeStamp: .zero,
                                                     duration: .invalid,
                                                     frameProperties: [kVTEncodeFrameOptionKey_ForceKeyFrame: true] as CFDictionary,
                                                     infoFlagsOut: nil) { s, _, sample in
            if s == noErr { output = sample }
        }
        VTCompressionSessionCompleteFrames(encoder, untilPresentationTimeStamp: .invalid)
        guard status == noErr, let sample = output, let format = CMSampleBufferGetFormatDescription(sample),
              let block = CMSampleBufferGetDataBuffer(sample) else { throw XCTSkip("encoding failed (\(status))") }
        var parameterSets: [Data] = []
        var count = 0
        CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: 0, parameterSetPointerOut: nil,
                                                           parameterSetSizeOut: nil, parameterSetCountOut: &count,
                                                           nalUnitHeaderLengthOut: nil)
        for i in 0..<count {
            var pointer: UnsafePointer<UInt8>?
            var size = 0
            CMVideoFormatDescriptionGetH264ParameterSetAtIndex(format, parameterSetIndex: i, parameterSetPointerOut: &pointer,
                                                               parameterSetSizeOut: &size, parameterSetCountOut: nil,
                                                               nalUnitHeaderLengthOut: nil)
            if let pointer { parameterSets.append(Data(bytes: pointer, count: size)) }
        }
        var length = 0
        var dataPointer: UnsafeMutablePointer<Int8>?
        CMBlockBufferGetDataPointer(block, atOffset: 0, lengthAtOffsetOut: nil, totalLengthOut: &length,
                                    dataPointerOut: &dataPointer)
        let raw = UnsafeRawBufferPointer(start: dataPointer, count: length)
        let annexB = try XCTUnwrap(AnnexB.fromLengthPrefixed(raw, parameterSets: parameterSets))
        return (annexB, width, height)
    }
}
