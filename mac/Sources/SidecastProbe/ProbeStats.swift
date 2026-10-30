import Foundation

/// Counters and per-second series for the probe's report.
struct ProbeStats {
    var configs = 0
    var seqGaps = 0
    private(set) var frames = 0
    private(set) var keyframes = 0
    private(set) var bytes = 0
    private(set) var rtts: [TimeInterval] = []
    private(set) var mbpsPerSecond: [Double] = []
    private(set) var fpsPerSecond: [Int] = []
    private(set) var startTime: TimeInterval = 0
    private var lastFrameTime: TimeInterval?
    private var lastKeyframeTime: TimeInterval?
    private var maxKeyframeGap: TimeInterval = 0
    private var windowFrames = 0
    private var windowBytes = 0
    private var windowKeyframes = 0
    private var lastSecondDecoded = 0

    mutating func start(at time: TimeInterval) { startTime = time }

    mutating func recordFrame(bytes n: Int, keyframe: Bool, at time: TimeInterval) {
        frames += 1
        bytes += n
        windowFrames += 1
        windowBytes += n
        if keyframe {
            keyframes += 1
            windowKeyframes += 1
            if let last = lastKeyframeTime { maxKeyframeGap = max(maxKeyframeGap, time - last) }
            lastKeyframeTime = time
        }
        lastFrameTime = time
    }

    mutating func recordRTT(_ rtt: TimeInterval) { rtts.append(rtt) }

    /// One line per second; resets the window.
    mutating func secondLine(decoder: ProbeDecoder?) -> String {
        let mbps = Double(windowBytes) * 8 / 1e6
        mbpsPerSecond.append(mbps)
        fpsPerSecond.append(windowFrames)
        let decoded = decoder?.decoded ?? 0
        let line = String(format: "probe: t=%2ds frames=%3d keyframes=%d Mbps=%6.2f decoded=%d decodeErrors=%d rtt=%@",
                          mbpsPerSecond.count, windowFrames, windowKeyframes, mbps, decoded - lastSecondDecoded,
                          decoder?.errors ?? 0, rtts.last.map { String(format: "%.1fms", $0 * 1000) } ?? "-")
        lastSecondDecoded = decoded
        windowFrames = 0
        windowBytes = 0
        windowKeyframes = 0
        return line
    }

    func summary(decoder: ProbeDecoder?) -> [String] {
        let duration = max(0.001, (lastFrameTime ?? startTime) - startTime)
        let avgRTT = rtts.isEmpty ? 0 : rtts.reduce(0, +) / Double(rtts.count)
        let series = mbpsPerSecond.map { String(format: "%.1f", $0) }.joined(separator: " ")
        var trend = "n/a"
        if mbpsPerSecond.count >= 3 {
            let first = mbpsPerSecond[1], last = mbpsPerSecond[mbpsPerSecond.count - 1]
            trend = String(format: "%.1f → %.1f Mbps (%@)", first, last,
                           last < first * 0.9 ? "decreasing" : last > first * 1.1 ? "increasing" : "steady")
        }
        return [
            String(format: "probe: SUMMARY frames=%d decoded=%d decodeErrors=%d sizeMismatches=%d keyframes=%d configs=%d seqGaps=%d",
                   frames, decoder?.decoded ?? 0, decoder?.errors ?? 0, decoder?.sizeMismatches ?? 0, keyframes, configs, seqGaps),
            String(format: "probe: SUMMARY avgFps=%.1f avgMbps=%.2f maxKeyframeGap=%.2fs rtt avg=%.1fms max=%.1fms (%d pongs)",
                   Double(frames) / duration, Double(bytes) * 8 / 1e6 / duration, maxKeyframeGap,
                   avgRTT * 1000, (rtts.max() ?? 0) * 1000, rtts.count),
            "probe: SUMMARY Mbps per second: \(series)",
            "probe: SUMMARY fps per second: \(fpsPerSecond.map(String.init).joined(separator: " "))",
            "probe: SUMMARY bitrate trend: \(trend)",
        ]
    }

    func problems(decoder: ProbeDecoder?, expectKeyframeResponse: Bool) -> [String] {
        var p: [String] = []
        if configs == 0 { p.append("no STREAM_CONFIG received") }
        if frames == 0 { p.append("no video frames received") }
        if let d = decoder {
            if d.decoded == 0 { p.append("no frame decoded") }
            if d.errors > 0 { p.append("\(d.errors) decode errors") }
            if d.sizeMismatches > 0 { p.append("\(d.sizeMismatches) decoded frames had the wrong size") }
        }
        if seqGaps > 0 { p.append("\(seqGaps) sequence gaps") }
        if rtts.isEmpty { p.append("no PONG received") }
        if maxKeyframeGap > 2.5 { p.append(String(format: "keyframe gap %.2fs exceeds 2 s", maxKeyframeGap)) }
        if expectKeyframeResponse { p.append("no keyframe after KEYFRAME_REQUEST") }
        return p
    }
}
