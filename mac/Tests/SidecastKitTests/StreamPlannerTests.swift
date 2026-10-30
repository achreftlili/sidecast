import XCTest
@testable import SidecastKit

final class StreamPlannerTests: XCTestCase {
    private let pixel8 = ScreenInfo(width: 2400, height: 1080, densityDpi: 420, refreshRate: 120)
    private let bothCodecs = [CodecInfo(name: "hevc", maxWidth: 3840, maxHeight: 2160),
                              CodecInfo(name: "h264", maxWidth: 1920, maxHeight: 1080)]

    func testDefaultHiDPIUsesFullPhonePixelsAtTwoX() throws {
        let plan = try StreamPlanner.plan(screen: pixel8, codecs: bothCodecs, settings: StreamSettings())
        XCTAssertEqual(plan.codec, .hevc)
        XCTAssertEqual(plan.display, VirtualDisplayMode(width: 1200, height: 540, hiDPI: true, refreshRate: 60))
        XCTAssertEqual(plan.display.pixelWidth, 2400)
        XCTAssertEqual(plan.encodeWidth, 2400)
        XCTAssertEqual(plan.encodeHeight, 1080)
        XCTAssertEqual(plan.fps, 60)
        XCTAssertEqual(plan.streamConfig, StreamConfig(codec: "hevc", width: 2400, height: 1080, fps: 60))
    }

    func testNonHiDPIAndScale() throws {
        var s = StreamSettings(hiDPI: false, resolutionScale: 0.75)
        var plan = try StreamPlanner.plan(screen: pixel8, codecs: bothCodecs, settings: s)
        XCTAssertEqual(plan.display, VirtualDisplayMode(width: 1800, height: 810, hiDPI: false, refreshRate: 60))
        XCTAssertEqual([plan.encodeWidth, plan.encodeHeight], [1800, 810])

        s = StreamSettings(hiDPI: true, resolutionScale: 0.5)
        plan = try StreamPlanner.plan(screen: pixel8, codecs: bothCodecs, settings: s)
        XCTAssertEqual(plan.display, VirtualDisplayMode(width: 600, height: 270, hiDPI: true, refreshRate: 60))
        XCTAssertEqual([plan.encodeWidth, plan.encodeHeight], [1200, 540])
    }

    func testOddSizesBecomeEven() throws {
        let odd = ScreenInfo(width: 2341, height: 1081, densityDpi: 400, refreshRate: 60)
        let plan = try StreamPlanner.plan(screen: odd, codecs: bothCodecs,
                                          settings: StreamSettings(hiDPI: false))
        XCTAssertEqual(plan.encodeWidth % 2, 0)
        XCTAssertEqual(plan.encodeHeight % 2, 0)
        XCTAssertEqual([plan.display.width, plan.display.height], [2340, 1080])
    }

    func testFallsBackToH264WhenHEVCCannotDecodeTheSize() throws {
        let codecs = [CodecInfo(name: "hevc", maxWidth: 1280, maxHeight: 720),
                      CodecInfo(name: "h264", maxWidth: 2560, maxHeight: 1440)]
        let plan = try StreamPlanner.plan(screen: pixel8, codecs: codecs, settings: StreamSettings())
        XCTAssertEqual(plan.codec, .h264)
        XCTAssertEqual([plan.encodeWidth, plan.encodeHeight], [2400, 1080])
    }

    func testClampsToLargestDecoderWhenNothingFits() throws {
        let codecs = [CodecInfo(name: "h264", maxWidth: 1920, maxHeight: 1080)]
        let plan = try StreamPlanner.plan(screen: pixel8, codecs: codecs, settings: StreamSettings())
        XCTAssertEqual(plan.codec, .h264)
        XCTAssertEqual(plan.encodeWidth, 1920)
        XCTAssertEqual(plan.encodeHeight, 864)       // 1920 × 1080/2400, aspect preserved
        // The virtual display keeps the phone's full mode; capture downsamples.
        XCTAssertEqual(plan.display.pixelWidth, 2400)
    }

    func testPortraitLimitsAreTreatedAsLandscape() throws {
        let codecs = [CodecInfo(name: "hevc", maxWidth: 1080, maxHeight: 1920)]
        let plan = try StreamPlanner.plan(screen: pixel8, codecs: codecs, settings: StreamSettings())
        XCTAssertEqual([plan.encodeWidth, plan.encodeHeight], [1920, 864])
    }

    func testClientPreferenceOrderWins() throws {
        let codecs = [CodecInfo(name: "h264", maxWidth: 3840, maxHeight: 2160),
                      CodecInfo(name: "hevc", maxWidth: 3840, maxHeight: 2160)]
        XCTAssertEqual(try StreamPlanner.plan(screen: pixel8, codecs: codecs, settings: StreamSettings()).codec, .h264)
    }

    func testUnknownCodecsAreIgnoredAndNoCommonCodecThrows() {
        let codecs = [CodecInfo(name: "av1", maxWidth: 3840, maxHeight: 2160)]
        XCTAssertThrowsError(try StreamPlanner.plan(screen: pixel8, codecs: codecs, settings: StreamSettings())) {
            XCTAssertEqual($0 as? StreamPlanError, .noCommonCodec)
        }
        XCTAssertThrowsError(try StreamPlanner.plan(screen: pixel8, codecs: [], settings: StreamSettings()))
    }

    func testFpsIsCappedByPanelRefreshAndHighFpsShrinksDecoderBudget() throws {
        let slowPanel = ScreenInfo(width: 2400, height: 1080, densityDpi: 420, refreshRate: 60)
        var plan = try StreamPlanner.plan(screen: slowPanel, codecs: bothCodecs, settings: StreamSettings(fps: 120))
        XCTAssertEqual(plan.fps, 60)

        // 120 fps on a 120 Hz panel: a 1920×1080@60 H.264 budget shrinks by √(60/120).
        let codecs = [CodecInfo(name: "h264", maxWidth: 1920, maxHeight: 1080)]
        plan = try StreamPlanner.plan(screen: pixel8, codecs: codecs, settings: StreamSettings(fps: 120))
        XCTAssertEqual(plan.fps, 120)
        XCTAssertLessThanOrEqual(plan.encodeWidth, 1358)
        XCTAssertLessThanOrEqual(plan.encodeHeight, 764)
        XCTAssertEqual(plan.display.refreshRate, 120)

        plan = try StreamPlanner.plan(screen: pixel8, codecs: bothCodecs, settings: StreamSettings(fps: 30))
        XCTAssertEqual(plan.fps, 30)
    }

    func testPortraitScreenIsNormalizedToLandscape() throws {
        let portrait = ScreenInfo(width: 1080, height: 2400, densityDpi: 420, refreshRate: 60)
        let plan = try StreamPlanner.plan(screen: portrait, codecs: bothCodecs, settings: StreamSettings())
        XCTAssertEqual([plan.encodeWidth, plan.encodeHeight], [2400, 1080])
    }

    func testInvalidValuesFallBackToDefaults() throws {
        let plan = try StreamPlanner.plan(screen: pixel8, codecs: bothCodecs,
                                          settings: StreamSettings(resolutionScale: 0.3, fps: 75))
        XCTAssertEqual(plan.fps, 60)
        XCTAssertEqual(plan.encodeWidth, 2400)
        XCTAssertThrowsError(try StreamPlanner.plan(screen: ScreenInfo(width: 0, height: 0, densityDpi: 1, refreshRate: 60),
                                                    codecs: bothCodecs, settings: StreamSettings()))
    }
}
