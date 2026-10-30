import Foundation

/// Monotonic time in seconds (injectable for tests).
public protocol TimeSource: Sendable {
    func now() -> TimeInterval
}

/// Mach uptime clock (same base as `DispatchTime` and capture timestamps).
public struct MonotonicTimeSource: TimeSource {
    public init() {}

    public func now() -> TimeInterval {
        TimeInterval(DispatchTime.now().uptimeNanoseconds) / 1_000_000_000
    }

    public static var nowMicros: UInt64 { DispatchTime.now().uptimeNanoseconds / 1000 }
}

/// A scheduled action that can be cancelled.
public protocol ScheduledTask: AnyObject {
    func cancel()
}

/// Runs delayed actions. Implementations call actions on the same serial
/// context the owner uses, so owners need no extra locking.
public protocol TaskScheduler: AnyObject {
    @discardableResult
    func schedule(after delay: TimeInterval, _ action: @escaping () -> Void) -> ScheduledTask
}

/// `TaskScheduler` on a serial dispatch queue.
public final class DispatchQueueScheduler: TaskScheduler {
    public let queue: DispatchQueue

    public init(queue: DispatchQueue) { self.queue = queue }

    private final class Item: ScheduledTask {
        let work: DispatchWorkItem
        init(_ work: DispatchWorkItem) { self.work = work }
        func cancel() { work.cancel() }
    }

    @discardableResult
    public func schedule(after delay: TimeInterval, _ action: @escaping () -> Void) -> ScheduledTask {
        let work = DispatchWorkItem(block: action)
        queue.asyncAfter(deadline: .now() + delay, execute: work)
        return Item(work)
    }
}

/// Deterministic scheduler for tests: actions run only when time is advanced.
public final class ManualScheduler: TaskScheduler, TimeSource, @unchecked Sendable {
    private final class Item: ScheduledTask {
        let due: TimeInterval
        let order: Int
        let action: () -> Void
        var cancelled = false
        init(due: TimeInterval, order: Int, action: @escaping () -> Void) {
            self.due = due
            self.order = order
            self.action = action
        }
        func cancel() { cancelled = true }
    }

    public private(set) var currentTime: TimeInterval
    private var items: [Item] = []
    private var counter = 0

    public init(start: TimeInterval = 0) { currentTime = start }

    public func now() -> TimeInterval { currentTime }

    public var pendingCount: Int { items.filter { !$0.cancelled }.count }

    @discardableResult
    public func schedule(after delay: TimeInterval, _ action: @escaping () -> Void) -> ScheduledTask {
        counter += 1
        let item = Item(due: currentTime + max(0, delay), order: counter, action: action)
        items.append(item)
        return item
    }

    /// Advances time, running due actions in order (including ones they schedule).
    public func advance(by delta: TimeInterval) {
        let target = currentTime + delta
        while true {
            items.removeAll { $0.cancelled }
            guard let next = items.filter({ $0.due <= target })
                .min(by: { ($0.due, $0.order) < ($1.due, $1.order) }) else { break }
            items.removeAll { $0 === next }
            currentTime = max(currentTime, next.due)
            next.action()
        }
        currentTime = target
    }
}
