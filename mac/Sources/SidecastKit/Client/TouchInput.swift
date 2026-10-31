import CoreGraphics
import Foundation

/// One pointer of an input event, in view coordinates (points on iOS).
public struct PointerSample: Equatable, Sendable {
    public var id: Int
    public var x: Double
    public var y: Double

    public init(id: Int, x: Double, y: Double) {
        self.id = id
        self.x = x
        self.y = y
    }
}

/// A touch event shaped like an Android `MotionEvent`, which is what TOUCH
/// carries (PROTOCOL §7): `actionIndex` indexes `pointers` (the pointer that
/// went down or up; 0 for MOVE and CANCEL).
public struct PointerEvent: Equatable, Sendable {
    public var action: TouchAction
    public var actionIndex: Int
    public var pointers: [PointerSample]

    public init(action: TouchAction, actionIndex: Int, pointers: [PointerSample]) {
        self.action = action
        self.actionIndex = actionIndex
        self.pointers = pointers
    }
}

/// A touch as UIKit reports it: a stable key (e.g. the `UITouch` identity) and its position.
public struct TrackedTouch<Key: Hashable>: Sendable where Key: Sendable {
    public var key: Key
    public var x: Double
    public var y: Double

    public init(key: Key, x: Double, y: Double) {
        self.key = key
        self.x = x
        self.y = y
    }
}

/// UIKit reports touches in sets that began, moved or ended together; TOUCH
/// follows Android: one DOWN / POINTER_DOWN per finger landing and one
/// POINTER_UP / UP per finger lifting, each with every pointer that is down,
/// and one MOVE with all of them. Pointer ids are small integers, the lowest
/// free one for each new finger (like Android), so the host's gesture engine
/// sees the same streams from both clients. Pure; not thread-safe.
public final class TouchSequencer<Key: Hashable & Sendable> {
    private struct Active {
        let key: Key
        let id: Int
        var x: Double
        var y: Double
    }

    /// Down pointers, sorted by id (Android's pointer-index order).
    private var active: [Active] = []

    public init() {}

    public var activeCount: Int { active.count }

    /// Fingers landed. `stillDown`: every touch UIKit says is down now (the
    /// event's `allTouches`); pointers missing from it lifted without an
    /// "ended" and are forgotten, so a missed end never leaves a ghost finger.
    public func began(_ touches: [TrackedTouch<Key>], stillDown: Set<Key>? = nil) -> [PointerEvent] {
        if let stillDown { active.removeAll { !stillDown.contains($0.key) } }
        var events: [PointerEvent] = []
        for touch in touches where !active.contains(where: { $0.key == touch.key }) {
            let ids = Set(active.map(\.id))
            let id = (0...).first { !ids.contains($0) }!
            let entry = Active(key: touch.key, id: id, x: touch.x, y: touch.y)
            let index = active.firstIndex { $0.id > id } ?? active.count
            active.insert(entry, at: index)
            events.append(PointerEvent(action: active.count == 1 ? .down : .pointerDown, actionIndex: index,
                                       pointers: samples()))
        }
        return events
    }

    /// Fingers moved: one MOVE with every down pointer (none for unknown touches).
    public func moved(_ touches: [TrackedTouch<Key>]) -> [PointerEvent] {
        var changed = false
        for touch in touches {
            guard let i = active.firstIndex(where: { $0.key == touch.key }) else { continue }
            active[i].x = touch.x
            active[i].y = touch.y
            changed = true
        }
        return changed ? [PointerEvent(action: .move, actionIndex: 0, pointers: samples())] : []
    }

    /// Fingers lifted: each one's event still lists it, at its last position.
    public func ended(_ touches: [TrackedTouch<Key>]) -> [PointerEvent] {
        var events: [PointerEvent] = []
        for touch in touches {
            guard let i = active.firstIndex(where: { $0.key == touch.key }) else { continue }
            active[i].x = touch.x
            active[i].y = touch.y
            events.append(PointerEvent(action: active.count == 1 ? .up : .pointerUp, actionIndex: i,
                                       pointers: samples()))
            active.remove(at: i)
        }
        return events
    }

    /// The system took the touches (e.g. a system gesture): the whole gesture is cancelled.
    public func cancelled() -> [PointerEvent] {
        guard !active.isEmpty else { return [] }
        let event = PointerEvent(action: .cancel, actionIndex: 0, pointers: samples())
        active.removeAll()
        return [event]
    }

    private func samples() -> [PointerSample] { active.map { PointerSample(id: $0.id, x: $0.x, y: $0.y) } }
}

/// Keeps pointers that must not reach the computer out of the forwarded touch
/// stream, and rewrites actions so what is forwarded stays a well-formed
/// gesture: its first pointer is a DOWN, its last lift an UP. Pure.
///
/// Why: a thumb resting on the screen edge while another finger taps would make
/// the host see a two-finger tap (= right click). A pointer is held back when it
/// goes down where `isIgnoredAt` says so (`first` = no other pointer is being
/// forwarded); it stays held back until it lifts.
public final class PointerFilter {
    private let isIgnoredAt: (_ x: Double, _ y: Double, _ first: Bool) -> Bool
    private var held = Set<Int>()
    /// Forwarded pointers currently down, in landing order.
    private var down: [Int] = []

    public init(isIgnoredAt: @escaping (_ x: Double, _ y: Double, _ first: Bool) -> Bool) {
        self.isIgnoredAt = isIgnoredAt
    }

    /// Ids held back right now (diagnostics).
    public var heldBack: Set<Int> { held }

    /// What to forward, or nil when the computer must not see this event.
    public func filter(_ event: PointerEvent) -> PointerEvent? {
        let actor = event.pointers.indices.contains(event.actionIndex) ? event.pointers[event.actionIndex] : nil
        switch event.action {
        case .down:
            reset()   // a new gesture: nothing from an earlier one survives a missed UP
            return actor.flatMap { pointerDown($0, event.pointers) }
        case .pointerDown:
            return actor.flatMap { pointerDown($0, event.pointers) }
        case .move:
            let forwarded = visible(event.pointers)
            return forwarded.isEmpty ? nil : PointerEvent(action: .move, actionIndex: 0, pointers: forwarded)
        case .pointerUp:
            return actor.flatMap { pointerUp($0, event.pointers) }
        case .up:
            defer { reset() }
            return actor.flatMap { pointerUp($0, event.pointers) }
        case .cancel:
            let forwarded = visible(event.pointers)
            reset()
            return forwarded.isEmpty ? nil : PointerEvent(action: .cancel, actionIndex: 0, pointers: forwarded)
        }
    }

    private func pointerDown(_ p: PointerSample, _ pointers: [PointerSample]) -> PointerEvent? {
        if isIgnoredAt(p.x, p.y, down.isEmpty) {
            held.insert(p.id)
            return nil
        }
        down.append(p.id)
        let forwarded = visible(pointers)
        return PointerEvent(action: down.count == 1 ? .down : .pointerDown,
                            actionIndex: forwarded.firstIndex { $0.id == p.id } ?? 0, pointers: forwarded)
    }

    private func pointerUp(_ p: PointerSample, _ pointers: [PointerSample]) -> PointerEvent? {
        if held.remove(p.id) != nil || !down.contains(p.id) { return nil }
        let forwarded = visible(pointers)
        down.removeAll { $0 == p.id }
        return PointerEvent(action: down.isEmpty ? .up : .pointerUp,
                            actionIndex: forwarded.firstIndex { $0.id == p.id } ?? 0, pointers: forwarded)
    }

    private func visible(_ pointers: [PointerSample]) -> [PointerSample] { pointers.filter { down.contains($0.id) } }

    private func reset() {
        held.removeAll()
        down.removeAll()
    }
}

/// Where the video is drawn inside its container, in container coordinates.
public struct VideoRect: Equatable, Sendable {
    public var x: Double
    public var y: Double
    public var width: Double
    public var height: Double

    public init(x: Double, y: Double, width: Double, height: Double) {
        self.x = x
        self.y = y
        self.width = width
        self.height = height
    }

    public static let zero = VideoRect(x: 0, y: 0, width: 0, height: 0)

    public var cgRect: CGRect { CGRect(x: x, y: y, width: width, height: height) }

    /// Container point → video-normalized point (0…1 inside the picture, beyond in the bars).
    public func normalizeX(_ px: Double) -> Float { width > 0 ? Float((px - x) / width) : 0 }
    public func normalizeY(_ py: Double) -> Float { height > 0 ? Float((py - y) / height) : 0 }

    /// Video-normalized point → container point (inverse of `normalizeX` / `normalizeY`).
    public func toContainerX(_ nx: Float) -> Double { x + Double(nx) * width }
    public func toContainerY(_ ny: Float) -> Double { y + Double(ny) * height }
}

/// Aspect-preserving fit ("letterbox" / "pillarbox"), shared by layout and touch mapping.
public enum Letterbox {
    /// `scale`: device pixels per container unit; the picture is snapped to whole pixels.
    public static func fit(containerWidth: Double, containerHeight: Double, videoWidth: Int, videoHeight: Int,
                           scale: Double = 1) -> VideoRect {
        guard containerWidth > 0, containerHeight > 0 else { return .zero }
        guard videoWidth > 0, videoHeight > 0 else {
            return VideoRect(x: 0, y: 0, width: containerWidth, height: containerHeight)
        }
        let s = max(scale, 1e-6)
        let k = min(containerWidth / Double(videoWidth), containerHeight / Double(videoHeight))
        let w = min(containerWidth, max(1 / s, (Double(videoWidth) * k * s).rounded() / s))
        let h = min(containerHeight, max(1 / s, (Double(videoHeight) * k * s).rounded() / s))
        let x = ((containerWidth - w) / 2 * s).rounded(.down) / s
        let y = ((containerHeight - h) / 2 * s).rounded(.down) / s
        return VideoRect(x: x, y: y, width: w, height: h)
    }
}

/// Pointer events → TOUCH (PROTOCOL §7). Pure.
public enum TouchTranslator {
    /// Nil when there are no pointers, or when the action concerns an 11th+
    /// pointer that the 10-pointer limit leaves out. `eventTimeMs` wraps.
    public static func translate(_ event: PointerEvent, eventTimeMs: UInt32, rect: VideoRect) -> TouchEvent? {
        guard !event.pointers.isEmpty else { return nil }
        let sent = Array(event.pointers.prefix(TouchEvent.maxPointers))
        let actor: PointerSample
        switch event.action {
        case .move, .cancel:
            actor = sent[0]
        default:
            guard sent.indices.contains(event.actionIndex) else { return nil }
            actor = sent[event.actionIndex]
        }
        return TouchEvent(eventTimeMs: eventTimeMs, action: event.action,
                          actionPointerId: UInt8(truncatingIfNeeded: actor.id),
                          pointers: sent.map {
                              TouchPointer(id: UInt8(truncatingIfNeeded: $0.id), x: rect.normalizeX($0.x),
                                           y: rect.normalizeY($0.y))
                          })
    }

    /// A timestamp in seconds since boot (UITouch / ProcessInfo uptime) as TOUCH `eventTimeMs`.
    public static func eventTimeMs(uptime seconds: TimeInterval) -> UInt32 {
        UInt32(truncatingIfNeeded: Int64(max(0, seconds) * 1000))
    }
}

/// Recognizes a quick three-finger tap (the client's stats toggle, §Q27). It
/// only observes: touches are still forwarded unchanged.
public final class ThreeFingerTapDetector {
    private let slop: Double
    private let maxDuration: TimeInterval
    private var downTime: TimeInterval = 0
    private var maxPointers = 0
    private var moved = false
    private var origins: [Int: (Double, Double)] = [:]

    public init(slop: Double, maxDuration: TimeInterval = 0.4) {
        self.slop = slop
        self.maxDuration = maxDuration
    }

    /// Feed every event; true on the UP that completes a three-finger tap.
    public func onEvent(_ event: PointerEvent, time: TimeInterval) -> Bool {
        switch event.action {
        case .down:
            origins.removeAll()
            downTime = time
            maxPointers = event.pointers.count
            moved = false
            remember(event.pointers)
        case .pointerDown:
            maxPointers = max(maxPointers, event.pointers.count)
            remember(event.pointers)
        case .move:
            for p in event.pointers {
                guard let (ox, oy) = origins[p.id] else { continue }
                if hypot(p.x - ox, p.y - oy) > slop { moved = true }
            }
        case .up:
            let tap = maxPointers == 3 && !moved && time - downTime <= maxDuration
            origins.removeAll()
            maxPointers = 0
            return tap
        case .cancel:
            origins.removeAll()
            maxPointers = 0
        case .pointerUp:
            break
        }
        return false
    }

    private func remember(_ pointers: [PointerSample]) {
        for p in pointers where origins[p.id] == nil { origins[p.id] = (p.x, p.y) }
    }
}

/// Where to draw the computer's pointer (PROTOCOL §7.3), in container
/// coordinates, or nil to hide it. Pure.
public enum PointerPlacement {
    /// - Parameters:
    ///   - cursor: the last CURSOR of this connection, nil before the first one
    ///   - mode: the pointer exists only in trackpad mode
    ///   - playing: the stream is up and visible (no connection / error overlay)
    ///   - rect: where the video is drawn (same mapping as touches)
    public static func position(cursor: CursorPosition?, mode: InputMode, playing: Bool, rect: VideoRect) -> CGPoint? {
        guard let cursor, cursor.isOnDisplay, mode == .trackpad, playing, rect.width > 0, rect.height > 0 else {
            return nil
        }
        return CGPoint(x: rect.toContainerX(cursor.x), y: rect.toContainerY(cursor.y))
    }
}
