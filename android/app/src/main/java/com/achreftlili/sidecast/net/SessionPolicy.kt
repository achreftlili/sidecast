package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.net.StreamClient.EndReason
import com.achreftlili.sidecast.protocol.Reason

/** Why the session is waiting to reconnect; drives the overlay message. */
enum class RetryCause {
    /** Was streaming, link died ("Connection lost — reconnecting…"). */
    CONNECTION_LOST,

    /** Never got through to the Mac yet. */
    UNREACHABLE,

    /** BYE server_shutdown: Sidecast on the Mac stopped. */
    SERVER_STOPPED,

    /** BYE/AUTH_RESULT unknown_device: pairing was deleted, pair again right away. */
    REPAIR,
}

/** Terminal problems that need the user (no automatic reconnect). */
sealed interface SessionError {
    data object Replaced : SessionError

    data object PermissionMissing : SessionError

    data object ProtocolMismatch : SessionError

    data object IdentityChanged : SessionError

    data object PairingExpired : SessionError

    data object TooManyAttempts : SessionError

    data object DisplayFailed : SessionError

    data class Protocol(val detail: String) : SessionError

    data class Rejected(val reason: String) : SessionError
}

/** What to do after a [StreamClient] ended. */
sealed interface NextStep {
    data object Stop : NextStep

    data class Retry(val cause: RetryCause, val immediate: Boolean = false) : NextStep

    data class Fail(val error: SessionError) : NextStep
}

object SessionPolicy {
    /** [everStreamed]: this session has streamed before, so failures read as "connection lost". */
    fun next(reason: EndReason, everStreamed: Boolean): NextStep = when (reason) {
        EndReason.ClosedByUser -> NextStep.Stop
        EndReason.ConnectionLost -> NextStep.Retry(RetryCause.CONNECTION_LOST)
        is EndReason.ConnectFailed ->
            NextStep.Retry(if (everStreamed) RetryCause.CONNECTION_LOST else RetryCause.UNREACHABLE)
        is EndReason.IdentityChanged -> NextStep.Fail(SessionError.IdentityChanged)
        is EndReason.ProtocolError -> NextStep.Fail(SessionError.Protocol(reason.detail))
        is EndReason.ServerEnded -> when (reason.reason) {
            Reason.UNKNOWN_DEVICE -> NextStep.Retry(RetryCause.REPAIR, immediate = true)
            Reason.SERVER_SHUTDOWN -> NextStep.Retry(RetryCause.SERVER_STOPPED)
            Reason.REPLACED -> NextStep.Fail(SessionError.Replaced)
            Reason.PERMISSION_MISSING -> NextStep.Fail(SessionError.PermissionMissing)
            Reason.PROTOCOL_MISMATCH -> NextStep.Fail(SessionError.ProtocolMismatch)
            Reason.DISPLAY_FAILED -> NextStep.Fail(SessionError.DisplayFailed)
            Reason.PAIRING_EXPIRED -> NextStep.Fail(SessionError.PairingExpired)
            Reason.TOO_MANY_ATTEMPTS -> NextStep.Fail(SessionError.TooManyAttempts)
            else -> NextStep.Fail(SessionError.Rejected(reason.reason))
        }
    }
}

/** §6: reconnect after 1 s, 2 s, 4 s, then every 5 s. */
object ReconnectBackoff {
    private const val BASE_MS = 1_000L
    private const val MAX_MS = 5_000L

    /** Delay before reconnect attempt number [failures] (1-based). */
    fun delayMs(failures: Int): Long {
        if (failures <= 1) return BASE_MS
        val shift = (failures - 1).coerceAtMost(10)
        return (BASE_MS shl shift).coerceAtMost(MAX_MS)
    }
}
