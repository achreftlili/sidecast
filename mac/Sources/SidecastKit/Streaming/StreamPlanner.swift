import Foundation

/// Codecs the Mac can encode.
public enum VideoCodec: String, CaseIterable, Sendable {
    case hevc
    case h264
}

/// User settings that shape the stream.
public struct StreamSettings: Equatable, Sendable {
    /// Render the virtual display at 2× (Retina-style) instead of 1×.
    public var hiDPI: Bool
    /// Fraction of the phone's pixels to use: 1.0, 0.75 or 0.5.
    public var resolutionScale: Double
    public var fps: Int
    public var maxBitrateMbps: Int
    /// "Phone controls the Mac": gates TOUCH and keyboard input (TEXT / KEY).
    public var touchEnabled: Bool

    public static let allowedScales: [Double] = [1.0, 0.75, 0.5]
    public static let allowedFps: [Int] = [30, 60, 120]
    public static let bitrateRangeMbps: ClosedRange<Int> = 10...100

    public init(hiDPI: Bool = true, resolutionScale: Double = 1.0, fps: Int = 60,
                maxBitrateMbps: Int = 50, touchEnabled: Bool = true) {
        self.hiDPI = hiDPI
        self.resolutionScale = resolutionScale
        self.fps = fps
        self.maxBitrateMbps = maxBitrateMbps
        self.touchEnabled = touchEnabled
    }
}

/// Bitrate ceiling and floor for one stream (PROTOCOL §5, §6): the user's
/// maximum capped by the client's limit; the 4 Mbps floor drops to the
/// ceiling when the limit is lower than that.
public struct BitrateBounds: Equatable, Sendable {
    public static let defaultFloor = 4_000_000
    public static let defaultStart = 20_000_000

    public var floor: Int
    public var ceiling: Int

    public init(settingMbps: Int, limits: StreamLimits?) {
        var ceiling = max(1, settingMbps) * 1_000_000
        if let cap = limits?.maxBitrate { ceiling = min(ceiling, max(100_000, cap)) }
        self.ceiling = ceiling
        self.floor = min(Self.defaultFloor, ceiling)
    }

    /// Where the adaptive controller starts: the usual 20 Mbps, clamped into the bounds.
    public var start: Int { min(max(Self.defaultStart, floor), ceiling) }
}

/// The virtual display to create (in macOS points; pixels = points × scale).
public struct VirtualDisplayMode: Equatable, Sendable {
    public var width: Int
    public var height: Int
    public var hiDPI: Bool
    public var refreshRate: Int

    public var pixelWidth: Int { hiDPI ? width * 2 : width }
    public var pixelHeight: Int { hiDPI ? height * 2 : height }
}

/// Everything negotiated for one stream.
public struct StreamPlan: Equatable, Sendable {
    public var codec: VideoCodec
    public var encodeWidth: Int
    public var encodeHeight: Int
    public var fps: Int
    public var display: VirtualDisplayMode

    public var streamConfig: StreamConfig {
        StreamConfig(codec: codec.rawValue, width: encodeWidth, height: encodeHeight, fps: fps)
    }
}

public enum StreamPlanError: Error, Equatable {
    case noCommonCodec
    case invalidScreen
}

/// Chooses the virtual display mode, codec and encode size for a client.
public enum StreamPlanner {
    /// Encoder ceilings on Apple Silicon (VideoToolbox).
    static let encoderMax: [VideoCodec: (width: Int, height: Int)] = [
        .hevc: (8192, 4320),
        .h264: (4096, 2304),
    ]
    /// Clients report decoder limits measured at this frame rate (PROTOCOL §5).
    static let limitReferenceFps = 60

    /// `limits` (HELLO data saver) caps the stream's frame rate: fps = min(setting,
    /// phone refresh, limit). The virtual display keeps the uncapped refresh rate.
    public static func plan(screen: ScreenInfo, codecs: [CodecInfo], settings: StreamSettings,
                            limits: StreamLimits? = nil) throws -> StreamPlan {
        // Landscape physical pixels (the client promises width ≥ height; be tolerant).
        let phoneW = max(screen.width, screen.height)
        let phoneH = min(screen.width, screen.height)
        guard phoneW >= 64, phoneH >= 64 else { throw StreamPlanError.invalidScreen }

        let scale = StreamSettings.allowedScales.contains(settings.resolutionScale) ? settings.resolutionScale : 1.0
        let displayFps = effectiveFps(setting: settings.fps, clientRefresh: screen.refreshRate)
        let fps = limitedFps(displayFps, limits: limits)

        // Virtual display: the scaled pixel size, as 2× points when HiDPI.
        let targetW = Double(phoneW) * scale
        let targetH = Double(phoneH) * scale
        let display: VirtualDisplayMode
        if settings.hiDPI {
            display = VirtualDisplayMode(width: max(1, Int((targetW / 2).rounded())),
                                         height: max(1, Int((targetH / 2).rounded())),
                                         hiDPI: true, refreshRate: displayFps)
        } else {
            display = VirtualDisplayMode(width: evenFloor(Int(targetW.rounded())),
                                         height: evenFloor(Int(targetH.rounded())),
                                         hiDPI: false, refreshRate: displayFps)
        }
        let wantW = display.pixelWidth
        let wantH = display.pixelHeight

        // Codec: the client's most preferred codec that fits the wanted size;
        // if none fits, the one with the largest budget (and we downscale).
        let candidates: [(VideoCodec, (width: Int, height: Int))] = codecs.compactMap { info in
            guard let codec = VideoCodec(rawValue: info.name.lowercased()),
                  info.maxWidth > 0, info.maxHeight > 0 else { return nil }
            let limit = scaleLimit((info.maxWidth, info.maxHeight), forFps: fps)
            let enc = encoderMax[codec]!
            let box = (width: min(max(limit.width, limit.height), enc.width),
                       height: min(min(limit.width, limit.height), enc.height))
            return (codec, box)
        }
        guard !candidates.isEmpty else { throw StreamPlanError.noCommonCodec }

        let chosen = candidates.first { fits(wantW, wantH, in: $0.1) }
            ?? candidates.max { $0.1.width * $0.1.height < $1.1.width * $1.1.height }!
        let size = clamp(width: wantW, height: wantH, into: chosen.1)
        return StreamPlan(codec: chosen.0, encodeWidth: size.width, encodeHeight: size.height,
                          fps: fps, display: display)
    }

    /// The fps setting, capped by the phone's refresh rate (never below 30).
    static func effectiveFps(setting: Int, clientRefresh: Double) -> Int {
        let wanted = StreamSettings.allowedFps.contains(setting) ? setting : 60
        guard clientRefresh.isFinite, clientRefresh > 0 else { return wanted }
        let panel = max(30, Int(clientRefresh.rounded()))
        return min(wanted, panel)
    }

    /// Lowest frame rate a client limit can ask for.
    public static let minLimitedFps = 5

    /// `fps` capped by the client's `maxFps` (never below `minLimitedFps`).
    public static func limitedFps(_ fps: Int, limits: StreamLimits?) -> Int {
        guard let cap = limits?.maxFps else { return fps }
        return min(fps, max(minLimitedFps, cap))
    }

    /// A decoder limit is measured at 60 fps. Faster streams spend the same
    /// decode budget on more frames, so shrink the area by 60/fps (ported
    /// from SideScreen's CodecLimits.scaleLimit).
    static func scaleLimit(_ limit: (width: Int, height: Int), forFps fps: Int) -> (width: Int, height: Int) {
        guard fps > limitReferenceFps else { return limit }
        let k = (Double(limitReferenceFps) / Double(fps)).squareRoot()
        return (evenFloor(Int(Double(limit.width) * k)), evenFloor(Int(Double(limit.height) * k)))
    }

    /// Whether a landscape size fits a landscape box.
    static func fits(_ w: Int, _ h: Int, in box: (width: Int, height: Int)) -> Bool {
        w <= box.width && h <= box.height
    }

    /// Scales (w, h) down to fit `box`, keeping the aspect ratio, with even dimensions.
    static func clamp(width: Int, height: Int, into box: (width: Int, height: Int)) -> (width: Int, height: Int) {
        guard !fits(width, height, in: box) else { return (evenFloor(width), evenFloor(height)) }
        let s = min(Double(box.width) / Double(width), Double(box.height) / Double(height))
        // The epsilon keeps e.g. 2400 × (1920/2400) from truncating to 1919.
        let w = evenFloor(min(box.width, Int(Double(width) * s + 1e-6)))
        let h = evenFloor(min(box.height, Int(Double(height) * s + 1e-6)))
        return (w, h)
    }

    static func evenFloor(_ v: Int) -> Int { max(2, v & ~1) }
}
