package com.achreftlili.sidecast.ui

import android.content.Context
import androidx.annotation.StringRes
import com.achreftlili.sidecast.R
import com.achreftlili.sidecast.net.RetryCause
import com.achreftlili.sidecast.net.Route
import com.achreftlili.sidecast.net.SessionError
import com.achreftlili.sidecast.net.SessionState
import com.achreftlili.sidecast.video.DecoderStatus

data class OverlayAction(val label: String, val onClick: () -> Unit)

/** Message layer drawn over the (possibly frozen) video. */
data class Overlay(
    val title: String,
    val detail: String? = null,
    val progress: Boolean = false,
    val primary: OverlayAction? = null,
    val secondary: OverlayAction? = null,
)

interface OverlayActions {
    fun retry()

    fun pairAgain()

    fun close()
}

/** Maps session + decoder state to what the display screen shows; null = show the video. */
class OverlayPresenter(
    private val context: Context,
    private val fallbackName: String,
    private val actions: OverlayActions,
) {
    /** [route]: the path being tried / used, shown while connecting (§1.1). */
    fun present(state: SessionState, decoder: DecoderStatus, route: Route? = null): Overlay? = when (state) {
        SessionState.Idle, SessionState.Paused -> busy(s(R.string.display_connecting, fallbackName), via(route))
        is SessionState.Connecting -> busy(s(R.string.display_connecting, state.macName), via(route))
        is SessionState.Pairing -> busy(s(R.string.display_pairing, state.macName), s(R.string.display_pairing_detail))
        is SessionState.AwaitingCode -> Overlay(s(R.string.display_enter_code), secondary = cancel())
        is SessionState.VerifyingCode -> busy(s(R.string.display_verifying))
        is SessionState.Streaming -> streaming(decoder)
        is SessionState.Reconnecting -> reconnecting(state, route)
        is SessionState.Failed -> failed(state.macName, state.error)
        SessionState.Stopped -> null
    }

    private fun streaming(decoder: DecoderStatus): Overlay? = when (decoder) {
        is DecoderStatus.Running -> if (decoder.hasPicture) null else busy(s(R.string.display_starting))
        is DecoderStatus.Unsupported -> Overlay(
            title = s(R.string.error_decoder_title),
            detail = s(
                R.string.error_decoder_detail,
                decoder.config.codec.uppercase(),
                decoder.config.width,
                decoder.config.height,
                decoder.detail,
            ),
            primary = OverlayAction(s(R.string.action_retry), actions::retry),
            secondary = close(),
        )
        DecoderStatus.Idle -> busy(s(R.string.display_starting))
    }

    private fun via(route: Route?): String? = when (route) {
        Route.LAN -> s(R.string.display_via_lan)
        Route.TAILSCALE -> s(R.string.display_via_tailscale)
        Route.INTERNET -> s(R.string.display_via_internet)
        null -> null
    }

    private fun reconnecting(state: SessionState.Reconnecting, route: Route?): Overlay {
        val title = when (state.cause) {
            RetryCause.CONNECTION_LOST -> s(R.string.display_connection_lost)
            RetryCause.UNREACHABLE -> s(R.string.display_unreachable, state.macName)
            RetryCause.SERVER_STOPPED -> s(R.string.display_server_stopped, state.macName)
            RetryCause.REPAIR -> s(R.string.display_repairing, state.macName)
        }
        val hint = when {
            state.cause == RetryCause.UNREACHABLE -> s(R.string.display_unreachable_detail)
            state.attempt > 1 -> s(R.string.display_attempt, state.attempt)
            else -> null
        }
        val detail = listOfNotNull(via(route), hint).joinToString("\n").ifEmpty { null }
        return Overlay(title, detail, progress = true, secondary = close())
    }

    private fun failed(name: String, error: SessionError): Overlay {
        val retry = OverlayAction(s(R.string.action_retry), actions::retry)
        return when (error) {
            SessionError.Replaced -> Overlay(
                s(R.string.error_replaced_title),
                s(R.string.error_replaced_detail, name),
                primary = OverlayAction(s(R.string.action_reconnect), actions::retry),
                secondary = close(),
            )
            SessionError.PermissionMissing ->
                Overlay(s(R.string.error_permission_title), s(R.string.error_permission_detail), primary = retry, secondary = close())
            SessionError.ProtocolMismatch ->
                Overlay(s(R.string.error_protocol_mismatch_title), s(R.string.error_protocol_mismatch_detail), secondary = close())
            SessionError.IdentityChanged -> Overlay(
                s(R.string.error_identity_title),
                s(R.string.error_identity_detail, name),
                primary = OverlayAction(s(R.string.action_pair_again), actions::pairAgain),
                secondary = close(),
            )
            SessionError.PairingExpired -> Overlay(
                s(R.string.error_pairing_expired_title),
                s(R.string.error_pairing_restart_detail),
                primary = OverlayAction(s(R.string.action_try_again), actions::retry),
                secondary = close(),
            )
            SessionError.TooManyAttempts -> Overlay(
                s(R.string.error_too_many_attempts_title),
                s(R.string.error_pairing_restart_detail),
                primary = OverlayAction(s(R.string.action_try_again), actions::retry),
                secondary = close(),
            )
            SessionError.DisplayFailed ->
                Overlay(s(R.string.error_display_failed_title), s(R.string.error_display_failed_detail, name), primary = retry, secondary = close())
            is SessionError.Protocol ->
                Overlay(s(R.string.error_protocol_title), s(R.string.error_protocol_detail, error.detail), primary = retry, secondary = close())
            is SessionError.Rejected ->
                Overlay(s(R.string.error_rejected_title), s(R.string.error_rejected_detail, error.reason), primary = retry, secondary = close())
        }
    }

    private fun busy(title: String, detail: String? = null) = Overlay(title, detail, progress = true, secondary = cancel())

    private fun cancel() = OverlayAction(s(R.string.action_cancel), actions::close)

    private fun close() = OverlayAction(s(R.string.action_close), actions::close)

    private fun s(@StringRes id: Int, vararg args: Any): String = context.getString(id, *args)
}
