import SidecastMicDriver
import XCTest

/// The lock-free sample-time ring shared by the driver's two devices.
final class RingTests: XCTestCase {
    private var ring: OpaquePointer!

    override func setUp() {
        ring = scm_ring_create()
        XCTAssertNotNil(ring)
    }

    override func tearDown() {
        scm_ring_destroy(ring)
    }

    private func write(_ t: Int64, _ samples: [Float]) {
        samples.withUnsafeBufferPointer { scm_ring_write(ring, t, $0.baseAddress, UInt32($0.count)) }
    }

    private func read(_ t: Int64, _ count: Int) -> (samples: [Float], fed: UInt32) {
        var out = [Float](repeating: .nan, count: count)
        let fed = out.withUnsafeMutableBufferPointer { scm_ring_read(ring, t, $0.baseAddress, UInt32(count)) }
        return (out, fed)
    }

    func testWrittenFramesReadBackAtTheSameSampleTimes() {
        let samples = (0..<512).map { Float($0) / 512 }
        write(1000, samples)
        let r = read(1000, 512)
        XCTAssertEqual(r.samples, samples)
        XCTAssertEqual(r.fed, 512)
        // An offset read sees the overlap only.
        let shifted = read(1256, 512)
        XCTAssertEqual(Array(shifted.samples[0..<256]), Array(samples[256...]))
        XCTAssertEqual(Array(shifted.samples[256...]), [Float](repeating: 0, count: 256))
        XCTAssertEqual(shifted.fed, 256)
    }

    func testNothingFedReadsSilence() {
        let r = read(0, 1024)
        XCTAssertEqual(r.samples, [Float](repeating: 0, count: 1024))
        XCTAssertEqual(r.fed, 0)
        // Including sample time 0 and negative times (fresh slots are tagged empty, not 0).
        XCTAssertEqual(read(-64, 128).fed, 0)
    }

    func testStaleAudioFromAnEarlierLapIsNeverReturned() {
        let frames = Int64(SCM_RING_FRAMES)
        write(500, [Float](repeating: 0.5, count: 100))
        // One full lap later the same slots hold old data, but for other sample times.
        let lapLater = read(500 + frames, 100)
        XCTAssertEqual(lapLater.fed, 0)
        XCTAssertEqual(lapLater.samples, [Float](repeating: 0, count: 100))
        // A newer write to the same slots replaces the old audio for good.
        write(500 + frames, [Float](repeating: -0.25, count: 100))
        XCTAssertEqual(read(500, 100).fed, 0)
        XCTAssertEqual(read(500 + frames, 100).samples, [Float](repeating: -0.25, count: 100))
    }

    func testWrapAroundTheEndOfTheRing() {
        let frames = Int64(SCM_RING_FRAMES)
        let samples = (0..<64).map { Float($0) }
        let t = 3 * frames - 32   // straddles a lap boundary
        write(t, samples)
        XCTAssertEqual(read(t, 64).samples, samples)
        XCTAssertEqual(read(t, 64).fed, 64)
    }

    func testPartiallyFedRangeMixesAudioAndSilence() {
        write(10_000, [1, 2, 3])
        write(10_005, [6, 7])
        XCTAssertEqual(read(10_000, 8).samples, [1, 2, 3, 0, 0, 6, 7, 0])
    }

    func testReinitClearsEverything() {
        write(42, [1, 1, 1])
        scm_ring_init(ring)
        XCTAssertEqual(read(42, 3).fed, 0)
    }

    /// Progress shared by the writer and reader threads.
    private final class Progress: @unchecked Sendable {
        private let lock = NSLock()
        private var _written = -1, _consumed = -1, _bad = 0
        var written: Int { get { lock.withLock { _written } } set { lock.withLock { _written = newValue } } }
        var consumed: Int { get { lock.withLock { _consumed } } set { lock.withLock { _consumed = newValue } } }
        var bad: Int { lock.withLock { _bad } }
        func addBad(_ n: Int) { lock.withLock { _bad += n } }
    }

    func testConcurrentWriterAndReaderNeverSeeTornOrStaleFrames() {
        // Like the HAL: the writer (feed output, ahead of now) stays a few blocks
        // ahead of the reader (microphone input, behind now) — never a full lap
        // (64 blocks) ahead. Every frame encodes its sample time, so the reader
        // can verify each value it gets.
        let blocks = 4000, block = 512, maxLead = 8
        let done = expectation(description: "reader finished")
        let ring = self.ring!
        let progress = Progress()
        DispatchQueue.global(qos: .userInteractive).async {
            var buffer = [Float](repeating: 0, count: block)
            var b = 0
            while b < blocks {
                if progress.written < b { continue }
                buffer.withUnsafeMutableBufferPointer { _ = scm_ring_read(ring, Int64(b * block), $0.baseAddress, UInt32(block)) }
                var bad = 0
                for i in 0..<block where buffer[i] != Float((b * block + i) % 4096) { bad += 1 }
                if bad > 0 { progress.addBad(bad) }
                progress.consumed = b
                b += 1
            }
            done.fulfill()
        }
        var buffer = [Float](repeating: 0, count: block)
        var b = 0
        while b < blocks {
            if b - progress.consumed > maxLead { continue }
            for i in 0..<block { buffer[i] = Float((b * block + i) % 4096) }
            buffer.withUnsafeBufferPointer { scm_ring_write(ring, Int64(b * block), $0.baseAddress, UInt32(block)) }
            progress.written = b
            b += 1
        }
        wait(for: [done], timeout: 60)
        XCTAssertEqual(progress.bad, 0)
    }
}
