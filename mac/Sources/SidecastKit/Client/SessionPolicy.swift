import Foundation

/// Why the session is waiting to reconnect; drives the overlay message.
public enum RetryCause: Equatable, Sendable {
    /// Was streaming and the link died ("Connection lost — reconnecting…").
    case connectionLost
    /// Never got through to the computer yet.
    case unreachable
    /// BYE server_shutdown: Sidecast on the computer stopped.
    case serverStopped
    /// BYE / AUTH_RESULT unknown_device: the pairing was deleted, pair again right away.
    case repair
}

/// Terminal problems that need the user (no automatic reconnect).
public enum SessionError: Equatable, Sendable {
    case replaced
    case permissionMissing
    case protocolMismatch
    case identityChanged
    case pairingExpired
    case tooManyAttempts
    case displayFailed
    case protocolError(String)
    case rejected(String)
}

/// What to do after a `StreamClient` ended.
public enum NextStep: Equatable, Sendable {
    case stop
    case retry(RetryCause, immediate: Bool)
    case fail(SessionError)
}

public enum SessionPolicy {
    /// `everStreamed`: this session has streamed before, so failures read as "connection lost".
    public static func next(after reason: StreamClient.EndReason, everStreamed: Bool) -> NextStep {
        switch reason {
        case .closedByUser: return .stop
        case .connectionLost: return .retry(.connectionLost, immediate: false)
        case .connectFailed: return .retry(everStreamed ? .connectionLost : .unreachable, immediate: false)
        case .identityChanged: return .fail(.identityChanged)
        case .protocolError(let detail): return .fail(.protocolError(detail))
        case .serverEnded(let raw):
            switch ByeReason(rawValue: raw) {
            case .unknownDevice: return .retry(.repair, immediate: true)
            case .serverShutdown: return .retry(.serverStopped, immediate: false)
            case .replaced: return .fail(.replaced)
            case .permissionMissing: return .fail(.permissionMissing)
            case .protocolMismatch: return .fail(.protocolMismatch)
            case .displayFailed: return .fail(.displayFailed)
            case .pairingExpired: return .fail(.pairingExpired)
            case .tooManyAttempts: return .fail(.tooManyAttempts)
            case .badCode, nil: return .fail(.rejected(raw))
            }
        }
    }
}

/// PROTOCOL §6: reconnect after 1 s, 2 s, 4 s, then every 5 s.
public enum ReconnectBackoff {
    /// Delay before reconnect attempt number `failures` (1-based).
    public static func delay(failures: Int) -> TimeInterval {
        guard failures > 1 else { return 1 }
        return min(5, pow(2, Double(min(failures - 1, 10))))
    }
}
