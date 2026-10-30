import XCTest
@testable import SidecastKit

final class AudioJitterBufferTests: XCTestCase {
    /// Small numbers so behaviour is easy to follow: 10-frame packets, play at 30, ceiling 60.
    private func makeBuffer() -> AudioJitterBuffer {
        AudioJitterBuffer(configuration: .init(targetFrames: 30, maxFrames: 60))
    }

    /// A packet whose samples encode (seq, index) so the output shows what was kept.
    private func packet(_ seq: UInt32, frames: Int = 10) -> AudioPacket {
        AudioPacket(seq: seq, samples: (0..<frames).map { Int16(Int(seq) * 100 + $0 + 1) })
    }

    private func read(_ b: inout AudioJitterBuffer, _ n: Int) -> [Float] {
        var out = [Float](repeating: .nan, count: n)
        out.withUnsafeMutableBufferPointer { _ = b.read(into: $0) }
        return out
    }

    /// Sample values back as the ints that were pushed (0 = silence).
    private func ints(_ floats: [Float]) -> [Int] { floats.map { Int(($0 * 32_768).rounded()) } }

    func testConvertsInt16ToFloat() {
        var b = AudioJitterBuffer(configuration: .init(targetFrames: 1, maxFrames: 100))
        b.push(AudioPacket(seq: 0, samples: [.min, 16_384, 0, -16_384, .max]))
        let out = read(&b, 5)
        XCTAssertEqual(out[0], -1)
        XCTAssertEqual(out[1], 0.5)
        XCTAssertEqual(out[2], 0)
        XCTAssertEqual(out[3], -0.5)
        XCTAssertEqual(out[4], 32_767 / 32_768)
    }

    func testBuffersUntilTargetThenPlaysInOrder() {
        var b = makeBuffer()
        b.push(packet(0))
        b.push(packet(1))
        XCTAssertEqual(ints(read(&b, 10)), [Int](repeating: 0, count: 10), "20 < 30 queued: still buffering")
        XCTAssertTrue(b.isBuffering)
        b.push(packet(2))
        XCTAssertEqual(ints(read(&b, 15)), Array(1...10) + Array(101...105))
        XCTAssertFalse(b.isBuffering)
        XCTAssertEqual(b.queuedFrames, 15)
    }

    func testUnderrunPlaysSilenceAndRebuffers() {
        var b = makeBuffer()
        for s: UInt32 in 0..<3 { b.push(packet(s)) }
        _ = read(&b, 25)
        let out = ints(read(&b, 10))
        XCTAssertEqual(out, Array(206...210) + [0, 0, 0, 0, 0], "the shortfall is silence")
        XCTAssertEqual(b.stats.underruns, 1)
        XCTAssertTrue(b.isBuffering)
        // Re-buffers to the target before playing again.
        b.push(packet(3))
        b.push(packet(4))
        XCTAssertEqual(ints(read(&b, 5)), [0, 0, 0, 0, 0])
        b.push(packet(5))
        XCTAssertEqual(ints(read(&b, 5)), Array(301...305))
    }

    func testNothingQueuedIsSilence() {
        var b = makeBuffer()
        XCTAssertEqual(ints(read(&b, 64)), [Int](repeating: 0, count: 64))
        XCTAssertEqual(b.stats.silentFrames, 64)
    }

    func testOverflowDropsTheOldestBackToTarget() {
        var b = makeBuffer()
        for s: UInt32 in 0..<7 { b.push(packet(s)) }   // 70 > 60 queued
        XCTAssertEqual(b.queuedFrames, 30)
        XCTAssertEqual(b.stats.droppedFrames, 40)
        // The newest 30 frames survive: packets 4, 5, 6.
        XCTAssertEqual(ints(read(&b, 30)), Array(401...410) + Array(501...510) + Array(601...610))
    }

    func testSequenceGapIsFilledWithSilence() {
        var b = makeBuffer()
        b.push(packet(0))
        b.push(packet(3))   // 1 and 2 never came
        XCTAssertEqual(b.stats.missingPackets, 2)
        XCTAssertEqual(b.stats.gapFillFrames, 20)
        XCTAssertEqual(b.queuedFrames, 40)
        XCTAssertEqual(ints(read(&b, 40)), Array(1...10) + [Int](repeating: 0, count: 20) + Array(301...310))
    }

    func testHugeGapIsCappedAndCannotFloodTheBuffer() {
        var b = makeBuffer()
        b.push(packet(0))
        b.push(AudioPacket(seq: 100_000, samples: [Int16](repeating: 7, count: 10)))
        XCTAssertEqual(b.stats.missingPackets, 99_999)
        XCTAssertEqual(b.stats.gapFillFrames, 60, "fill capped at the ceiling")
        XCTAssertEqual(b.queuedFrames, 30, "then trimmed back to the target")
        XCTAssertEqual(ints(read(&b, 30)), [Int](repeating: 0, count: 20) + [Int](repeating: 7, count: 10),
                       "the newest audio is kept, at the end")
    }

    func testBackwardsSeqIsARestartWithoutGapFill() {
        var b = makeBuffer()
        b.push(packet(50))
        b.push(packet(0))   // the phone restarted its numbering
        b.push(packet(1))
        XCTAssertEqual(b.stats.restarts, 1)
        XCTAssertEqual(b.stats.missingPackets, 0)
        XCTAssertEqual(b.queuedFrames, 30)
    }

    func testResetEmptiesAndForgetsTheSequence() {
        var b = makeBuffer()
        for s: UInt32 in 0..<4 { b.push(packet(s)) }
        b.reset()
        XCTAssertEqual(b.queuedFrames, 0)
        XCTAssertTrue(b.isBuffering)
        b.push(packet(0))   // seq 0 again after a new active:true: no restart, no gap
        XCTAssertEqual(b.stats.restarts, 0)
        XCTAssertEqual(b.stats.missingPackets, 0)
        let stats = b.takeStats()
        XCTAssertEqual(stats.packets, 5)
        XCTAssertEqual(b.stats, AudioJitterBuffer.Stats())
    }

    func testDefaultConfigurationIs40And150Milliseconds() {
        let c = AudioJitterBuffer.Configuration()
        XCTAssertEqual(c.targetFrames, 48_000 * 40 / 1000)
        XCTAssertEqual(c.maxFrames, 48_000 * 150 / 1000)
    }

    func testSteadyTenMillisecondPacketsNeverUnderrun() {
        var b = AudioJitterBuffer()
        var seq: UInt32 = 0
        // Prime with 40 ms, then alternate one 480-frame packet in / one 512-frame render out, rate-matched.
        for _ in 0..<4 { b.push(AudioPacket(seq: seq, samples: [Int16](repeating: 1000, count: 480))); seq += 1 }
        var pushed = 4 * 480, pulled = 0
        for _ in 0..<2000 {
            while pushed - pulled < 1_920 + 512 {
                b.push(AudioPacket(seq: seq, samples: [Int16](repeating: 1000, count: 480)))
                seq += 1
                pushed += 480
            }
            _ = read(&b, 512)
            pulled += 512
        }
        XCTAssertEqual(b.stats.underruns, 0)
        XCTAssertEqual(b.stats.droppedFrames, 0)
    }

    // MARK: - Sequence tracker

    func testSequenceTracker() {
        var t = AudioSequenceTracker()
        XCTAssertEqual(t.observe(5), .first)
        XCTAssertEqual(t.observe(6), .inOrder)
        XCTAssertEqual(t.observe(9), .gap(missing: 2))
        XCTAssertEqual(t.observe(3), .restart)
        XCTAssertEqual(t.observe(4), .inOrder)
        t.reset()
        XCTAssertEqual(t.observe(0), .first)
        // Wrap-around is in order, not a restart.
        var w = AudioSequenceTracker()
        _ = w.observe(.max)
        XCTAssertEqual(w.observe(0), .inOrder)
        XCTAssertEqual(w.observe(2), .gap(missing: 1))
    }
}
