import CoreGraphics
import Foundation

public enum MouseButton: Equatable, Sendable {
    case left
    case right
}

/// Phase information for scroll events (maps onto CGEvent scroll/momentum phases).
public enum ScrollPhase: Equatable, Sendable {
    case began
    case changed
    case ended
    case momentumBegan
    case momentum
    case momentumEnded
}

/// System navigation triggered by 3/4-finger swipes, injected as the Mission
/// Control keyboard shortcuts (Control + arrow). Directions follow a Mac
/// trackpad: content follows the fingers.
public enum NavigationAction: Equatable, Sendable, CaseIterable {
    /// Fingers moved right → the Space (desktop / full-screen app) to the left: Control+←.
    case spaceLeft
    /// Fingers moved left → the Space to the right: Control+→.
    case spaceRight
    /// Fingers moved up → Mission Control: Control+↑.
    case missionControl
    /// Fingers moved down → App Exposé: Control+↓.
    case appExpose

    /// The keyboard shortcut, as a KEY event (PROTOCOL §7.1 key codes).
    public var keyEvent: KeyEvent {
        switch self {
        case .spaceLeft: return KeyEvent(.left, modifiers: .control)
        case .spaceRight: return KeyEvent(.right, modifiers: .control)
        case .missionControl: return KeyEvent(.up, modifiers: .control)
        case .appExpose: return KeyEvent(.down, modifiers: .control)
        }
    }
}

/// Abstract output of the gesture engine, in display-local points
/// (origin = top-left of the virtual display).
public enum MouseAction: Equatable, Sendable {
    case move(to: CGPoint)
    case buttonDown(MouseButton, at: CGPoint)
    /// Pointer motion while the left button is held.
    case drag(to: CGPoint)
    case buttonUp(MouseButton, at: CGPoint)
    /// A complete left click; `count` is the click state (2 = double click).
    case click(at: CGPoint, count: Int)
    case rightClick(at: CGPoint)
    /// Pixel deltas; positive dy scrolls content down with the finger (natural).
    case scroll(dx: Double, dy: Double, at: CGPoint, phase: ScrollPhase)
    /// Pinch zoom step (positive = zoom in), injected as ⌘-scroll.
    case zoom(delta: Int, at: CGPoint)
    /// 3/4-finger swipe: one per gesture.
    case navigate(NavigationAction)

    /// Where this action puts the pointer (nil for scroll, zoom, navigation).
    public var pointerLocation: CGPoint? {
        switch self {
        case .move(let p), .drag(let p), .click(let p, _), .rightClick(let p), .buttonDown(_, let p), .buttonUp(_, let p):
            return p
        case .scroll, .zoom, .navigate:
            return nil
        }
    }
}

/// Gesture tuning, ported from SideScreen's GestureThresholds. Distances are in
/// phone dp; the engine converts them with `pointsPerDp` (1 dp ≈ 1/160 inch).
public struct GestureThresholds: Sendable {
    public var tapMaxDistance: Double = 15
    public var doubleTapMaxInterval: TimeInterval = 0.4
    public var doubleTapMaxDistance: Double = 20
    /// Held at least this long without moving: release = right click, move = drag.
    public var longPressTime: TimeInterval = 0.5
    public var scrollSensitivity: Double = 1.2
    public var pinchMinDistance: Double = 20
    public var zoomSensitivity: Double = 0.5
    /// Moves closer together than this (client time) are coalesced.
    public var minMoveInterval: TimeInterval = 0.008
    /// Momentum starts if the finger lifted within this time of its last move.
    public var momentumMaxIdle: TimeInterval = 0.05
    public var momentumTick: TimeInterval = 0.016
    public var momentumGain: Double = 3.0
    public var momentumDecay: Double = 0.92
    public var momentumStartThreshold: Double = 2.0
    public var momentumStopThreshold: Double = 0.5
    /// Two-finger tap (→ right click): first finger down to last finger up.
    public var twoFingerTapMaxDuration: TimeInterval = 0.3
    /// 3/4-finger swipe: centroid travel (dp, dominant axis) that triggers navigation.
    public var swipeMinDistance: Double = 60

    // Trackpad mode (PROTOCOL §7.3)
    /// A tap: down to up within this, and within the tap slop.
    public var trackpadTapMaxDuration: TimeInterval = 0.25
    /// A second touch this soon after a tap's release is a double tap (or tap-and-hold drag).
    public var trackpadDoubleTapInterval: TimeInterval = 0.35
    /// Cursor points per finger point at slow speed: a full phone-width swipe ≈ 60 % of the display.
    public var trackpadBaseGain: Double = 0.6
    /// Finger speeds (dp/s) between which the gain ramps up smoothly to `× trackpadMaxAcceleration`.
    public var trackpadSlowSpeed: Double = 150
    public var trackpadFastSpeed: Double = 1_500
    public var trackpadMaxAcceleration: Double = 2.5

    /// Human-like click delays (move → down, down → up).
    public var clickTiming = ClickTiming()

    public init() {}
}
