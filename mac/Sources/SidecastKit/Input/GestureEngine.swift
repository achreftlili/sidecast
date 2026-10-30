import CoreGraphics
import Foundation

/// Turns TOUCH events into abstract mouse actions (ported from SideScreen's
/// gesture state machine):
///
/// - tap → click; second tap nearby within 0.4 s → double click
/// - hold ≥ 0.5 s, release → right click; hold ≥ 0.5 s, then move → left drag
/// - one-finger move → scroll (with momentum after a flick)
/// - two-finger move → scroll (with momentum); two-finger pinch → zoom
/// - two-finger tap (all fingers up within 0.3 s, no scroll/pinch) → right
///   click where the first finger landed (where the cursor already is)
/// - 3 or 4 fingers: any 1/2-finger gesture ends (scroll ended without
///   momentum, button released, no click); a centroid swipe past 60 dp emits
///   exactly one `.navigate` for the whole gesture — left → Space to the
///   right, right → Space to the left, up → Mission Control, down → App
///   Exposé; a 3-finger tap does nothing (the phone uses it locally)
///
/// Trackpad mode (`setMode(.trackpad)`, PROTOCOL §7.3) keeps a cursor of its
/// own and moves it by finger deltas × an accelerated gain: tap = click at the
/// cursor, double tap = double click, double-tap-and-hold then move = drag,
/// two-finger tap = right click at the cursor, two-finger move/pinch = scroll/
/// zoom at the cursor, 3/4-finger swipes as in touch mode, no long press.
///
/// Durations use the client's `eventTimeMs` (immune to network jitter);
/// momentum ticks run on the injected scheduler. Not thread-safe: call it
/// from the scheduler's serial context. `reset()` releases a held button.
public final class GestureEngine {
    /// Optional diagnostics: why a click/right click was decided (never called on hot paths).
    public var trace: ((String) -> Void)?

    private enum State: Equatable {
        case idle
        case pending            // one finger down, gesture undecided
        case scrolling          // one-finger scroll
        case dragging           // long press + move: left button held
        case twoFingerPending
        case twoFingerScroll
        case pinching
        case twoFingerTapPending // one of two tapping fingers lifted; right click if the other lifts in time
        case multiFinger         // 3+ fingers: swipe navigation only, until all fingers lift
        case waitingForRelease   // a multi-finger gesture ended; ignore until all fingers lift
        // Trackpad mode, one finger
        case trackpadPending     // down, moving the cursor; still a tap if it lifts quickly in place
        case trackpadPointing    // moved past the slop: only moves the cursor
        case trackpadTapHold     // second touch right after a tap: double tap, or drag once it moves
        case trackpadDragging    // left button held, cursor follows the finger
    }

    public var thresholds: GestureThresholds
    private let scheduler: TaskScheduler
    private let output: (MouseAction) -> Void

    /// Size of the target display in points.
    public private(set) var displaySize: CGSize = .zero
    /// Display points per phone dp; scales the distance thresholds.
    public private(set) var pointsPerDp: Double = 1

    private var state: State = .idle
    private var primaryId: UInt8 = 0
    private var startPoint = CGPoint.zero
    private var lastPoint = CGPoint.zero
    private var startTime: UInt32 = 0
    private var lastMoveTime: UInt32 = 0
    private var scrollBegan = false
    private var velocity = (dx: 0.0, dy: 0.0)          // scroll units per second
    private var lastTap: (time: UInt32, point: CGPoint)?

    private var startMidpoint = CGPoint.zero
    private var lastMidpoint = CGPoint.zero
    private var initialPinchDistance = 0.0
    private var lastPinchDistance = 0.0
    private var zoomRemainder = 0.0

    private var momentumTask: ScheduledTask?
    private var momentumStep = (dx: 0.0, dy: 0.0)      // per tick
    private var momentumPoint = CGPoint.zero
    private var momentumStarted = false

    // Two-finger tap
    private var twoFingerTapEligible = false
    private var secondaryClickPoint = CGPoint.zero
    private var tapRemaining: (id: UInt8, point: CGPoint)?

    // 3/4-finger swipe
    private var swipeTravel = (dx: 0.0, dy: 0.0)       // display points
    private var lastCentroid = CGPoint.zero
    private var swipeIds: [UInt8] = []
    private var navigated = false

    // Trackpad mode
    public private(set) var mode: InputMode = .touch
    /// The system cursor, display-local, if it is on this display (trackpad mode starts from it).
    public var cursorProvider: (() -> CGPoint?)?
    /// Trackpad cursor (display-local points); meaningful once `hasCursor`.
    public private(set) var cursor = CGPoint.zero
    /// The trackpad cursor has been placed (first trackpad touch).
    public var hasCursor: Bool { cursorInitialized }
    private var cursorInitialized = false
    private var cursorAtDown = CGPoint.zero
    private var fingerSpeed = 0.0                      // dp/s, smoothed

    public init(scheduler: TaskScheduler, thresholds: GestureThresholds = GestureThresholds(),
                output: @escaping (MouseAction) -> Void) {
        self.scheduler = scheduler
        self.thresholds = thresholds
        self.output = output
    }

    /// True while the engine holds the left button down (drag in progress).
    public var isButtonHeld: Bool { state == .dragging || state == .trackpadDragging }

    /// Switches touch ↔ trackpad. A real change ends the current gesture cleanly
    /// (button released, scroll ended, momentum stopped) and forgets taps.
    public func setMode(_ newMode: InputMode) {
        guard newMode != mode else { return }
        reset()
        mode = newMode
    }

    public func configure(displaySize: CGSize, pointsPerDp: Double = 1) {
        self.displaySize = displaySize
        self.pointsPerDp = pointsPerDp > 0 && pointsPerDp.isFinite ? pointsPerDp : 1
    }

    public func handle(_ event: TouchEvent) {
        guard displaySize.width > 0, displaySize.height > 0, !event.pointers.isEmpty,
              let action = event.touchAction else { return }
        switch action {
        case .down: mode == .trackpad ? trackpadDown(event) : fingerDown(event)
        case .move:
            if state == .multiFinger {
                multiFingerMove(event)
            } else if state == .twoFingerTapPending {
                twoFingerTapMove(event)
            } else if isTwoFingerState, event.pointers.count >= 2 {
                twoFingerMove(event)
            } else if isTrackpadOneFingerState {
                trackpadMove(event)
            } else if isOneFingerState {
                oneFingerMove(event)
            }
        case .up: isTrackpadOneFingerState ? trackpadUp(event) : lastFingerUp(event)
        case .pointerDown: pointerDown(event)
        case .pointerUp: pointerUp(event)
        case .cancel:
            stopMomentum()
            endCurrentGesture()
        }
    }

    /// Ends any gesture: releases a held button, stops momentum, forgets taps.
    /// Call on disconnect so no button stays stuck (SideScreen #71).
    public func reset() {
        stopMomentum()
        endCurrentGesture()
        lastTap = nil
    }

    // MARK: - One finger

    private var isOneFingerState: Bool { [.pending, .scrolling, .dragging].contains(state) }
    private var isTwoFingerState: Bool { [.twoFingerPending, .twoFingerScroll, .pinching].contains(state) }

    private func fingerDown(_ event: TouchEvent) {
        stopMomentum()
        if state != .idle { endCurrentGesture() }   // a missed UP must not leave a button held
        let pointer = event.pointers.first { $0.id == event.actionPointerId } ?? event.pointers[0]
        let p = point(pointer)
        primaryId = pointer.id
        startPoint = p
        lastPoint = p
        startTime = event.eventTimeMs
        lastMoveTime = event.eventTimeMs
        velocity = (0, 0)
        state = .pending
        output(.move(to: p))
    }

    private func oneFingerMove(_ event: TouchEvent) {
        let p = point(primaryPointer(event))
        let t = event.eventTimeMs
        let dt = elapsed(lastMoveTime, t)
        guard dt >= thresholds.minMoveInterval else { return }   // coalesce; delta accumulates

        switch state {
        case .pending:
            guard distance(p, startPoint) > slop(thresholds.tapMaxDistance) else { break }
            if elapsed(startTime, t) >= thresholds.longPressTime {
                state = .dragging
                output(.buttonDown(.left, at: startPoint))
                output(.drag(to: p))
            } else {
                state = .scrolling
                scrollBegan = false
                emitScroll(dx: p.x - lastPoint.x, dy: p.y - lastPoint.y, at: p, dt: dt)
            }
        case .scrolling:
            emitScroll(dx: p.x - lastPoint.x, dy: p.y - lastPoint.y, at: p, dt: dt)
        case .dragging:
            output(.drag(to: p))
        default:
            break
        }
        lastPoint = p
        lastMoveTime = t
    }

    private func lastFingerUp(_ event: TouchEvent) {
        let p = point(primaryPointer(event))
        let t = event.eventTimeMs
        switch state {
        case .pending:
            guard distance(p, startPoint) <= slop(thresholds.tapMaxDistance) else { break }
            if elapsed(startTime, t) >= thresholds.longPressTime {
                trace?("right click: long press \(Int(elapsed(startTime, t) * 1000)) ms")
                output(.rightClick(at: p))
            } else {
                trace?("tap after \(Int(elapsed(startTime, t) * 1000)) ms")
                tap(at: p, time: t)
            }
        case .scrolling:
            output(.scroll(dx: 0, dy: 0, at: p, phase: .ended))
            startMomentumIfFlicked(at: p, upTime: t)
        case .dragging:
            output(.drag(to: p))
            output(.buttonUp(.left, at: p))
        case .twoFingerScroll:
            output(.scroll(dx: 0, dy: 0, at: anchor(lastMidpoint), phase: .ended))
        case .twoFingerTapPending:
            if let tap = tapRemaining, elapsed(startTime, t) <= thresholds.twoFingerTapMaxDuration,
               distance(p, tap.point) <= slop(thresholds.tapMaxDistance) {
                trace?("right click: two-finger tap \(Int(elapsed(startTime, t) * 1000)) ms")
                if mode == .trackpad { cursor = secondaryClickPoint }
                output(.rightClick(at: secondaryClickPoint))
                lastTap = nil
            }
        default:
            break   // .multiFinger: the swipe (if any) already navigated; a tap does nothing
        }
        tapRemaining = nil
        state = .idle
    }

    private func tap(at p: CGPoint, time: UInt32) {
        if let last = lastTap,
           elapsed(last.time, time) <= thresholds.doubleTapMaxInterval,
           distance(p, last.point) <= slop(thresholds.doubleTapMaxDistance) {
            output(.click(at: last.point, count: 2))
            lastTap = nil   // a third tap starts over
        } else {
            output(.click(at: p, count: 1))
            lastTap = (time, p)
        }
    }

    // MARK: - Two fingers

    private func pointerDown(_ event: TouchEvent) {
        if state == .multiFinger {
            rebaselineSwipe(event.pointers)   // a 4th finger: no jump in the centroid
            return
        }
        if event.pointers.count >= 3 {
            beginMultiFinger(event.pointers)
            return
        }
        guard event.pointers.count >= 2 else { return }
        if isTwoFingerState {
            rebaseline(event.pointers[0], event.pointers[1], time: event.eventTimeMs)
            return
        }
        // Only an undecided one-finger touch can turn into a two-finger tap;
        // in trackpad mode it right-clicks at the cursor (where it was at touch-down).
        twoFingerTapEligible = state == .pending || state == .trackpadPending
        secondaryClickPoint = mode == .trackpad ? cursorAtDown : lastPoint
        stopMomentum()
        endCurrentGesture()
        let (a, b) = (event.pointers[0], event.pointers[1])
        rebaseline(a, b, time: event.eventTimeMs)
        startMidpoint = lastMidpoint
        initialPinchDistance = lastPinchDistance
        state = .twoFingerPending
    }

    private func twoFingerMove(_ event: TouchEvent) {
        let p1 = point(event.pointers[0]), p2 = point(event.pointers[1])
        let d = distance(p1, p2)
        let mid = midpoint(p1, p2)
        let t = event.eventTimeMs
        let dt = elapsed(lastMoveTime, t)
        guard dt >= thresholds.minMoveInterval else { return }

        if state == .twoFingerPending {
            if abs(d - initialPinchDistance) > slop(thresholds.pinchMinDistance) {
                state = .pinching
                zoomRemainder = 0
            } else if distance(mid, startMidpoint) > slop(thresholds.tapMaxDistance) {
                state = .twoFingerScroll
                scrollBegan = false
                velocity = (0, 0)
            }
        }
        switch state {
        case .pinching:
            zoomRemainder += (d - lastPinchDistance) * thresholds.zoomSensitivity
            let step = Int(zoomRemainder)           // toward zero; the remainder carries over
            if step != 0 {
                output(.zoom(delta: step, at: anchor(mid)))
                zoomRemainder -= Double(step)
            }
        case .twoFingerScroll:
            emitScroll(dx: mid.x - lastMidpoint.x, dy: mid.y - lastMidpoint.y, at: anchor(mid), dt: dt)
        default:
            break
        }
        lastPinchDistance = d
        lastMidpoint = mid
        lastMoveTime = t
    }

    private func pointerUp(_ event: TouchEvent) {
        let remaining = event.pointers.filter { $0.id != event.actionPointerId }
        if state == .multiFinger {
            if !remaining.isEmpty { rebaselineSwipe(remaining) }
            return   // stays a multi-finger gesture until the last finger lifts
        }
        if isTwoFingerState, remaining.count >= 2 {
            rebaseline(remaining[0], remaining[1], time: event.eventTimeMs)
            return
        }
        switch state {
        case .twoFingerPending:
            if twoFingerTapEligible, let finger = remaining.first,
               elapsed(startTime, event.eventTimeMs) <= thresholds.twoFingerTapMaxDuration {
                tapRemaining = (finger.id, point(finger))
                state = .twoFingerTapPending
                return
            }
        case .twoFingerScroll:
            output(.scroll(dx: 0, dy: 0, at: anchor(lastMidpoint), phase: .ended))
            startMomentumIfFlicked(at: anchor(lastMidpoint), upTime: event.eventTimeMs)
        case .dragging, .scrolling, .trackpadDragging:
            endCurrentGesture()
        default:
            break
        }
        state = .waitingForRelease
    }

    /// The finger still down after a quick two-finger tap must not wander off.
    private func twoFingerTapMove(_ event: TouchEvent) {
        guard let tap = tapRemaining else { return }
        let finger = event.pointers.first { $0.id == tap.id } ?? event.pointers[0]
        if distance(point(finger), tap.point) > slop(thresholds.tapMaxDistance) {
            tapRemaining = nil
            state = .waitingForRelease
        }
    }

    // MARK: - Three or four fingers

    private func beginMultiFinger(_ pointers: [TouchPointer]) {
        stopMomentum()
        endCurrentGesture()   // scroll ended without momentum, held button released, no click
        lastTap = nil
        tapRemaining = nil
        twoFingerTapEligible = false
        navigated = false
        swipeTravel = (0, 0)
        rebaselineSwipe(pointers)
        state = .multiFinger
    }

    /// Restarts centroid tracking for a new finger set, keeping the travel so far.
    private func rebaselineSwipe(_ pointers: [TouchPointer]) {
        lastCentroid = centroid(pointers)
        swipeIds = pointers.map(\.id).sorted()
    }

    private func multiFingerMove(_ event: TouchEvent) {
        guard event.pointers.map(\.id).sorted() == swipeIds else {
            return rebaselineSwipe(event.pointers)   // finger set changed: never count the centroid jump
        }
        let c = centroid(event.pointers)
        swipeTravel.dx += c.x - lastCentroid.x
        swipeTravel.dy += c.y - lastCentroid.y
        lastCentroid = c
        guard !navigated else { return }   // one navigation per gesture
        let ax = abs(swipeTravel.dx), ay = abs(swipeTravel.dy)
        guard max(ax, ay) > slop(thresholds.swipeMinDistance) else { return }
        navigated = true
        // Content follows the fingers, as on a trackpad.
        let action: NavigationAction = ax >= ay
            ? (swipeTravel.dx < 0 ? .spaceRight : .spaceLeft)
            : (swipeTravel.dy < 0 ? .missionControl : .appExpose)
        output(.navigate(action))
    }

    private func centroid(_ pointers: [TouchPointer]) -> CGPoint {
        guard !pointers.isEmpty else { return .zero }
        var x = 0.0, y = 0.0
        for pointer in pointers {
            let q = point(pointer)
            x += q.x
            y += q.y
        }
        return CGPoint(x: x / Double(pointers.count), y: y / Double(pointers.count))
    }

    /// Restarts distance/midpoint tracking (finger set changed) without a jump.
    private func rebaseline(_ a: TouchPointer, _ b: TouchPointer, time: UInt32) {
        let p1 = point(a), p2 = point(b)
        lastPinchDistance = distance(p1, p2)
        lastMidpoint = midpoint(p1, p2)
        lastMoveTime = time
    }

    // MARK: - Scrolling and momentum

    private func emitScroll(dx: Double, dy: Double, at p: CGPoint, dt: TimeInterval) {
        let sx = dx * thresholds.scrollSensitivity
        let sy = dy * thresholds.scrollSensitivity
        output(.scroll(dx: sx, dy: sy, at: p, phase: scrollBegan ? .changed : .began))
        scrollBegan = true
        if dt > 0, dt < 0.1 {
            let inst = (dx: sx / dt, dy: sy / dt)
            velocity = velocity == (0, 0) ? inst : (0.6 * inst.dx + 0.4 * velocity.dx, 0.6 * inst.dy + 0.4 * velocity.dy)
        } else {
            velocity = (0, 0)   // after a pause the old speed means nothing
        }
    }

    private func startMomentumIfFlicked(at p: CGPoint, upTime: UInt32) {
        guard elapsed(lastMoveTime, upTime) <= thresholds.momentumMaxIdle else { return }
        let k = thresholds.momentumTick * thresholds.momentumGain
        let step = (dx: velocity.dx * k, dy: velocity.dy * k)
        guard max(abs(step.dx), abs(step.dy)) > thresholds.momentumStartThreshold else { return }
        momentumStep = step
        momentumPoint = p
        momentumStarted = false
        scheduleMomentumTick()
    }

    private func scheduleMomentumTick() {
        momentumTask = scheduler.schedule(after: thresholds.momentumTick) { [weak self] in
            self?.momentumTick()
        }
    }

    private func momentumTick() {
        momentumTask = nil
        guard max(abs(momentumStep.dx), abs(momentumStep.dy)) >= thresholds.momentumStopThreshold else {
            finishMomentum()
            return
        }
        output(.scroll(dx: momentumStep.dx, dy: momentumStep.dy, at: momentumPoint,
                       phase: momentumStarted ? .momentum : .momentumBegan))
        momentumStarted = true
        momentumStep = (momentumStep.dx * thresholds.momentumDecay, momentumStep.dy * thresholds.momentumDecay)
        scheduleMomentumTick()
    }

    private func finishMomentum() {
        if momentumStarted {
            output(.scroll(dx: 0, dy: 0, at: momentumPoint, phase: .momentumEnded))
        }
        momentumStarted = false
        momentumStep = (0, 0)
    }

    private func stopMomentum() {
        momentumTask?.cancel()
        momentumTask = nil
        finishMomentum()
    }

    /// Closes the current gesture cleanly (button up, scroll ended) and goes idle.
    private func endCurrentGesture() {
        switch state {
        case .dragging: output(.buttonUp(.left, at: lastPoint))
        case .scrolling: output(.scroll(dx: 0, dy: 0, at: lastPoint, phase: .ended))
        case .twoFingerScroll: output(.scroll(dx: 0, dy: 0, at: anchor(lastMidpoint), phase: .ended))
        case .trackpadDragging: output(.buttonUp(.left, at: cursor))
        default: break
        }
        state = .idle
    }

    // MARK: - Trackpad mode, one finger

    private var isTrackpadOneFingerState: Bool {
        [.trackpadPending, .trackpadPointing, .trackpadTapHold, .trackpadDragging].contains(state)
    }

    /// Where two-finger scroll/zoom act: the cursor in trackpad mode, the fingers in touch mode.
    private func anchor(_ fingers: CGPoint) -> CGPoint { mode == .trackpad ? cursor : fingers }

    /// Picks up the system cursor if it is on this display; otherwise keeps ours (centre at first).
    private func syncCursor() {
        let w = Double(displaySize.width), h = Double(displaySize.height)
        if let p = cursorProvider?(), p.x >= 0, p.y >= 0, p.x < w, p.y < h {
            cursor = p
            cursorInitialized = true
        } else if !cursorInitialized {
            cursor = CGPoint(x: w / 2, y: h / 2)
            cursorInitialized = true
        }
        cursor = clampToDisplay(cursor)
    }

    private func trackpadDown(_ event: TouchEvent) {
        stopMomentum()
        if state != .idle { endCurrentGesture() }
        syncCursor()
        let pointer = event.pointers.first { $0.id == event.actionPointerId } ?? event.pointers[0]
        let p = point(pointer)
        primaryId = pointer.id
        startPoint = p
        lastPoint = p
        startTime = event.eventTimeMs
        lastMoveTime = event.eventTimeMs
        cursorAtDown = cursor
        fingerSpeed = 0
        if let last = lastTap, elapsed(last.time, event.eventTimeMs) <= thresholds.trackpadDoubleTapInterval {
            state = .trackpadTapHold
        } else {
            lastTap = nil
            state = .trackpadPending
        }
    }

    private func trackpadMove(_ event: TouchEvent) {
        let p = point(primaryPointer(event))
        let t = event.eventTimeMs
        let dt = elapsed(lastMoveTime, t)
        guard dt >= thresholds.minMoveInterval else { return }   // coalesce; delta accumulates
        let beyondSlop = distance(p, startPoint) > slop(thresholds.tapMaxDistance)
        switch state {
        case .trackpadPending:
            // The cursor follows at once (precise nudges); a tap snaps back to where it started.
            if beyondSlop { state = .trackpadPointing }
            moveCursor(dx: p.x - lastPoint.x, dy: p.y - lastPoint.y, dt: dt)
            output(.move(to: cursor))
        case .trackpadPointing:
            moveCursor(dx: p.x - lastPoint.x, dy: p.y - lastPoint.y, dt: dt)
            output(.move(to: cursor))
        case .trackpadTapHold:
            // Undecided: keep the cursor still (a double tap clicks where the first tap did).
            // Point and time both stay at touch-down, so the travel applied once it moves
            // is timed over the whole hold (no false acceleration from the jump).
            guard beyondSlop else { return }
            state = .trackpadDragging
            lastTap = nil
            trace?("trackpad: double-tap-and-hold → drag")
            output(.buttonDown(.left, at: cursor))
            moveCursor(dx: p.x - lastPoint.x, dy: p.y - lastPoint.y, dt: dt)
            output(.drag(to: cursor))
        case .trackpadDragging:
            moveCursor(dx: p.x - lastPoint.x, dy: p.y - lastPoint.y, dt: dt)
            output(.drag(to: cursor))
        default:
            break
        }
        lastPoint = p
        lastMoveTime = t
    }

    private func trackpadUp(_ event: TouchEvent) {
        let p = point(primaryPointer(event))
        let t = event.eventTimeMs
        let ms = Int(elapsed(startTime, t) * 1000)
        let isTap = elapsed(startTime, t) <= thresholds.trackpadTapMaxDuration
            && distance(p, startPoint) <= slop(thresholds.tapMaxDistance)
        switch state {
        case .trackpadPending where isTap:
            cursor = cursorAtDown
            trace?("trackpad tap after \(ms) ms")
            output(.click(at: cursor, count: 1))
            lastTap = (t, cursor)
        case .trackpadTapHold where isTap:
            cursor = cursorAtDown
            trace?("trackpad double tap after \(ms) ms")
            output(.click(at: cursor, count: 2))
            lastTap = nil   // a third tap starts over
        case .trackpadDragging:
            output(.buttonUp(.left, at: cursor))
            lastTap = nil
        default:
            if state == .trackpadPending { trace?("trackpad: no click (held \(ms) ms)") }
            lastTap = nil   // pointing, a slow touch, or a slow second touch: no click (no long press)
        }
        state = .idle
    }

    /// Moves the cursor by a finger delta (display points) with pointer acceleration.
    private func moveCursor(dx: Double, dy: Double, dt: TimeInterval) {
        let speed = dt > 0 ? hypot(dx, dy) / pointsPerDp / dt : 0
        fingerSpeed = fingerSpeed == 0 ? speed : 0.5 * speed + 0.5 * fingerSpeed
        let gain = Self.trackpadGain(speedDpPerSecond: fingerSpeed, thresholds: thresholds)
        cursor = clampToDisplay(CGPoint(x: cursor.x + dx * gain, y: cursor.y + dy * gain))
    }

    /// Pointer acceleration: the base gain up to the slow speed, rising smoothly
    /// (smoothstep) to base × max acceleration at the fast speed. Never decreases with speed.
    public static func trackpadGain(speedDpPerSecond v: Double, thresholds: GestureThresholds) -> Double {
        let lo = thresholds.trackpadSlowSpeed, hi = max(thresholds.trackpadFastSpeed, lo + 1)
        let x = min(max((v - lo) / (hi - lo), 0), 1)
        let s = x * x * (3 - 2 * x)
        return thresholds.trackpadBaseGain * (1 + (thresholds.trackpadMaxAcceleration - 1) * s)
    }

    private func clampToDisplay(_ p: CGPoint) -> CGPoint {
        let w = Double(displaySize.width), h = Double(displaySize.height)
        return CGPoint(x: min(max(p.x, 0), max(0, w - 1)), y: min(max(p.y, 0), max(0, h - 1)))
    }

    // MARK: - Geometry

    private func primaryPointer(_ event: TouchEvent) -> TouchPointer {
        event.pointers.first { $0.id == primaryId } ?? event.pointers[0]
    }

    /// Normalized → display-local points, clamped onto the display.
    private func point(_ p: TouchPointer) -> CGPoint {
        func clamp(_ v: Float) -> Double { v.isFinite ? min(max(Double(v), 0), 1) : 0 }
        let w = Double(displaySize.width), h = Double(displaySize.height)
        return CGPoint(x: min(clamp(p.x) * w, max(0, w - 1)), y: min(clamp(p.y) * h, max(0, h - 1)))
    }

    private func slop(_ dp: Double) -> Double { dp * pointsPerDp }

    private func distance(_ a: CGPoint, _ b: CGPoint) -> Double { hypot(a.x - b.x, a.y - b.y) }

    private func midpoint(_ a: CGPoint, _ b: CGPoint) -> CGPoint {
        CGPoint(x: (a.x + b.x) / 2, y: (a.y + b.y) / 2)
    }

    /// Seconds between two wrapping millisecond timestamps.
    private func elapsed(_ from: UInt32, _ to: UInt32) -> TimeInterval {
        TimeInterval(to &- from) / 1000
    }
}
