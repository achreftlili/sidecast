import XCTest
@testable import SidecastKit

final class InFlightTrackerTests: XCTestCase {
    func testAtMostFourInFlightIncludingEncoding() {
        var t = InFlightTracker()
        XCTAssertTrue(t.tryReserve())
        XCTAssertEqual(t.markSent(at: 0), 0)
        XCTAssertTrue(t.tryReserve())
        XCTAssertEqual(t.markSent(at: 0), 1)
        XCTAssertTrue(t.tryReserve())          // still encoding
        XCTAssertTrue(t.tryReserve())
        XCTAssertEqual(t.inFlight, 4)
        XCTAssertFalse(t.tryReserve())         // 5th frame is dropped before encoding
        t.cancelReservation()                  // encoder produced nothing for one
        XCTAssertTrue(t.tryReserve())
    }

    func testAckReturnsRTTAndFreesSlots() {
        var t = InFlightTracker()
        for i in 0..<4 {
            XCTAssertTrue(t.tryReserve())
            XCTAssertEqual(t.markSent(at: Double(i)), UInt32(i))
        }
        XCTAssertFalse(t.tryReserve())
        XCTAssertEqual(t.ack(0, at: 0.5)!, 0.5, accuracy: 1e-9)
        XCTAssertEqual(t.inFlight, 3)
        XCTAssertNil(t.ack(0, at: 0.6))        // duplicate
        XCTAssertTrue(t.tryReserve())
    }

    func testAcksAreCumulative() {
        var t = InFlightTracker()
        for _ in 0..<4 { _ = t.tryReserve(); _ = t.markSent(at: 0) }
        XCTAssertNotNil(t.ack(2, at: 1))       // 0 and 1 implicitly acked
        XCTAssertEqual(t.sentUnacked, 1)
        XCTAssertNotNil(t.ack(3, at: 1))
        XCTAssertEqual(t.sentUnacked, 0)
    }

    func testUnknownFutureAckDoesNotCrash() {
        var t = InFlightTracker()
        _ = t.tryReserve(); _ = t.markSent(at: 0)
        XCTAssertNil(t.ack(1000, at: 1))
        XCTAssertEqual(t.sentUnacked, 0)
    }
}

final class AdaptiveBitrateTests: XCTestCase {
    private let mbps = 1_000_000

    func testDefaultsStartAt20Mbps() {
        let c = AdaptiveBitrateController(now: 0)
        XCTAssertEqual(c.bitrate, 20 * mbps)
        XCTAssertEqual(c.increaseStep, 2_500_000)   // 5 % of 50 Mbps
    }

    func testDropDecreasesBy25PercentDownToMin() {
        var c = AdaptiveBitrateController(config: .init(warmupAcks: 0), now: 0)
        var expected = 20.0 * Double(mbps)
        for second in 1...10 {
            c.recordRTT(0.005)
            c.recordDrop()
            XCTAssertEqual(c.tick(now: Double(second)), .decrease(reason: "1 dropped"))
            expected = max(4.0 * Double(mbps), expected * 0.75)
            XCTAssertEqual(c.bitrate, Int(expected), accuracy: 2)
        }
        XCTAssertEqual(c.bitrate, 4 * mbps)
    }

    func testHighSmoothedRTTDecreases() {
        var c = AdaptiveBitrateController(now: 0)
        c.recordRTT(0.005)
        XCTAssertEqual(c.tick(now: 1), .hold)
        for _ in 0..<40 { c.recordRTT(0.120) }    // srtt converges well above min + 40 ms
        XCTAssertEqual(c.tick(now: 2), .decrease(reason: "rtt"))
        XCTAssertEqual(c.bitrate, 15 * mbps)
    }

    func testIncreaseAfterTwoCleanWindowsCappedAtMax() {
        var c = AdaptiveBitrateController(config: .init(maxBitrate: 30 * mbps), now: 0)
        XCTAssertEqual(c.increaseStep, 1_500_000)      // max(1 Mbps, 5 % of 30 Mbps)
        var now = 0.0
        func cleanWindow() -> AdaptiveBitrateController.Decision? {
            c.recordRTT(0.004)
            now += 1
            return c.tick(now: now)
        }
        XCTAssertEqual(cleanWindow(), .hold)
        XCTAssertEqual(cleanWindow(), .increase)
        XCTAssertEqual(c.bitrate, 21_500_000)
        XCTAssertEqual(cleanWindow(), .hold)
        XCTAssertEqual(cleanWindow(), .increase)
        for _ in 0..<40 { _ = cleanWindow() }
        XCTAssertEqual(c.bitrate, 30 * mbps)
    }

    func testMinimumStepIsOneMbps() {
        let c = AdaptiveBitrateController(config: .init(startBitrate: 5 * mbps, maxBitrate: 10 * mbps), now: 0)
        XCTAssertEqual(c.increaseStep, 1 * mbps)
    }

    func testDropResetsCleanStreak() {
        var c = AdaptiveBitrateController(config: .init(warmupAcks: 0), now: 0)
        c.recordRTT(0.004); _ = c.tick(now: 1)              // clean #1
        c.recordDrop(); c.recordRTT(0.004); _ = c.tick(now: 2) // congested
        XCTAssertEqual(c.bitrate, 15 * mbps)
        c.recordRTT(0.004)
        XCTAssertEqual(c.tick(now: 3), .hold)                // clean #1 again, no increase yet
        c.recordRTT(0.004)
        XCTAssertEqual(c.tick(now: 4), .increase)
    }

    func testDropsDuringWarmUpAreIgnored() {
        var c = AdaptiveBitrateController(config: .init(warmupAcks: 3), now: 0)
        c.recordDrop(count: 3)                   // before any ack
        c.recordRTT(0.004); c.recordRTT(0.004)
        c.recordDrop()                           // 2 acks: still warming up
        XCTAssertEqual(c.tick(now: 1), .hold)
        XCTAssertEqual(c.bitrate, 20 * mbps)
        c.recordRTT(0.004)
        c.recordDrop()
        XCTAssertEqual(c.tick(now: 2), .decrease(reason: "1 dropped"))
    }

    func testWindowWithoutAcksHolds() {
        var c = AdaptiveBitrateController(now: 0)
        XCTAssertEqual(c.tick(now: 1), .hold)
        XCTAssertEqual(c.tick(now: 2), .hold)
        XCTAssertEqual(c.bitrate, 20 * mbps)
    }

    func testTickBeforeWindowEndsReturnsNil() {
        var c = AdaptiveBitrateController(now: 10)
        XCTAssertNil(c.tick(now: 10.5))
        XCTAssertNotNil(c.tick(now: 11.0))
    }

    func testLoweringMaxClampsImmediately() {
        var c = AdaptiveBitrateController(now: 0)
        c.setMaxBitrate(10 * mbps)
        XCTAssertEqual(c.bitrate, 10 * mbps)
        c.setMaxBitrate(1 * mbps)                // below min: max is raised to min
        XCTAssertEqual(c.bitrate, 4 * mbps)
    }
}
