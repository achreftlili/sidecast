package com.achreftlili.sidecast.input

import com.achreftlili.sidecast.protocol.Touch

/**
 * Keeps pointers that must not reach the Mac out of the forwarded touch stream, and rewrites the
 * actions so what is forwarded stays a well-formed gesture: its first pointer is a DOWN, its last
 * lift an UP. Pure, so it is unit tested.
 *
 * Why: the stream container handles a gesture itself once its first finger lands on the video, so
 * every later finger goes to it too, even one landing on a floating button or the mic chip. A thumb
 * resting on the screen edge while another finger taps is the same: the Mac would see a two-finger
 * tap (= right click). A pointer is held back when it goes down where [isIgnoredAt] says so
 * (`first` = no other pointer is being forwarded); it stays held back until it lifts.
 */
class PointerFilter(private val isIgnoredAt: (x: Float, y: Float, first: Boolean) -> Boolean) {
    /** A forwarded event: [actionIndex] indexes [pointers] (only forwarded ones, in event order). */
    data class Event(val actionMasked: Int, val actionIndex: Int, val pointers: List<PointerSample>)

    private val held = HashSet<Int>()
    private val down = LinkedHashSet<Int>() // forwarded pointers currently down

    /** Ids held back right now (for diagnostics). */
    val heldBack: Set<Int> get() = held

    /** @return what to forward, or null when the Mac must not see this event. */
    fun filter(actionMasked: Int, actionIndex: Int, pointers: List<PointerSample>): Event? {
        val actor = pointers.getOrNull(actionIndex)
        return when (actionMasked) {
            Touch.ACTION_DOWN -> {
                reset() // a new gesture: nothing from an earlier one survives a missed UP
                actor?.let { pointerDown(it, pointers) }
            }
            Touch.ACTION_POINTER_DOWN -> actor?.let { pointerDown(it, pointers) }
            Touch.ACTION_MOVE -> visible(pointers).takeIf { it.isNotEmpty() }?.let { Event(Touch.ACTION_MOVE, 0, it) }
            Touch.ACTION_POINTER_UP -> actor?.let { pointerUp(it, pointers) }
            Touch.ACTION_UP -> actor?.let { pointerUp(it, pointers) }.also { reset() }
            Touch.ACTION_CANCEL -> {
                val forwarded = visible(pointers)
                reset()
                if (forwarded.isEmpty()) null else Event(Touch.ACTION_CANCEL, 0, forwarded)
            }
            else -> null
        }
    }

    private fun pointerDown(p: PointerSample, pointers: List<PointerSample>): Event? {
        if (isIgnoredAt(p.x, p.y, down.isEmpty())) {
            held += p.id
            return null
        }
        down += p.id
        val forwarded = visible(pointers)
        val action = if (down.size == 1) Touch.ACTION_DOWN else Touch.ACTION_POINTER_DOWN
        return Event(action, forwarded.indexOfFirst { it.id == p.id }, forwarded)
    }

    private fun pointerUp(p: PointerSample, pointers: List<PointerSample>): Event? {
        if (held.remove(p.id) || p.id !in down) return null
        val forwarded = visible(pointers)
        down -= p.id
        val action = if (down.isEmpty()) Touch.ACTION_UP else Touch.ACTION_POINTER_UP
        return Event(action, forwarded.indexOfFirst { it.id == p.id }, forwarded)
    }

    private fun visible(pointers: List<PointerSample>) = pointers.filter { it.id in down }

    private fun reset() {
        held.clear()
        down.clear()
    }
}
