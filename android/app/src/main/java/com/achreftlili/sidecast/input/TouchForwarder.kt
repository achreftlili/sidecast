package com.achreftlili.sidecast.input

import android.annotation.SuppressLint
import android.view.MotionEvent
import android.view.View
import com.achreftlili.sidecast.protocol.Touch

/**
 * Turns MotionEvents on the full-screen container into TOUCH messages normalized to the
 * letterboxed video rect. Every MOVE is forwarded (no batching; historical samples are not
 * expanded). Pointers that go down on the floating controls, or extra pointers in the edge band,
 * are held back ([PointerFilter]); the local three-finger tap still sees every finger.
 */
class TouchForwarder(
    slopPx: Float,
    private val videoRect: () -> VideoRect,
    private val send: (Touch) -> Unit,
    private val onThreeFingerTap: () -> Unit,
    isIgnoredAt: (x: Float, y: Float, first: Boolean) -> Boolean = { _, _, _ -> false },
    private val log: (String) -> Unit = {},
) : View.OnTouchListener {
    private val tapDetector = ThreeFingerTapDetector(slopPx)
    private val filter = PointerFilter(isIgnoredAt)

    // The container is not a control: touches are a remote pointer, so performClick does not apply.
    @SuppressLint("ClickableViewAccessibility")
    override fun onTouch(view: View, event: MotionEvent): Boolean {
        val pointers = List(event.pointerCount) { i -> PointerSample(event.getPointerId(i), event.getX(i), event.getY(i)) }
        val action = event.actionMasked
        val forwarded = filter.filter(action, event.actionIndex, pointers)
        if (forwarded != null) {
            TouchTranslator.translate(forwarded.actionMasked, forwarded.actionIndex, event.eventTime, forwarded.pointers, videoRect())
                ?.let(send)
        } else if (action == MotionEvent.ACTION_DOWN || action == MotionEvent.ACTION_POINTER_DOWN) {
            val p = pointers[event.actionIndex]
            log("held back pointer ${p.id} at (${p.x.toInt()}, ${p.y.toInt()})")
        }
        if (tapDetector.onEvent(action, event.eventTime, pointers)) onThreeFingerTap()
        return true
    }
}
