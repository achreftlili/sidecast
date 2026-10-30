package com.achreftlili.sidecast.input

import com.achreftlili.sidecast.protocol.Protocol
import com.achreftlili.sidecast.protocol.Touch
import com.achreftlili.sidecast.protocol.TouchPointer

/** One pointer of an input event, in container pixels. */
data class PointerSample(val id: Int, val x: Float, val y: Float)

/** MotionEvent data -> TOUCH message (§7). Pure so it can be unit tested without Android. */
object TouchTranslator {
    private val FORWARDED_ACTIONS = setOf(
        Touch.ACTION_DOWN, Touch.ACTION_UP, Touch.ACTION_MOVE, Touch.ACTION_CANCEL,
        Touch.ACTION_POINTER_DOWN, Touch.ACTION_POINTER_UP,
    )

    /**
     * @param actionMasked MotionEvent.getActionMasked()
     * @param actionIndex MotionEvent.getActionIndex() (pointer the DOWN/UP refers to)
     * @param eventTimeMs MotionEvent.getEventTime() (uptime ms; sent as wrapping u32)
     * @param pointers all pointers of the event in index order
     * @return null for actions the protocol does not carry, or when the action concerns an 11th+
     *   pointer that the 10-pointer limit leaves out
     */
    fun translate(
        actionMasked: Int,
        actionIndex: Int,
        eventTimeMs: Long,
        pointers: List<PointerSample>,
        rect: VideoRect,
    ): Touch? {
        if (actionMasked !in FORWARDED_ACTIONS || pointers.isEmpty()) return null
        val sent = pointers.take(Protocol.MAX_TOUCH_POINTERS)
        val actionPointerId = when (actionMasked) {
            Touch.ACTION_MOVE, Touch.ACTION_CANCEL -> sent[0].id
            else -> {
                if (actionIndex !in sent.indices) return null
                sent[actionIndex].id
            }
        }
        return Touch(
            eventTimeMs = eventTimeMs and 0xFFFF_FFFFL,
            action = actionMasked,
            actionPointerId = actionPointerId and 0xFF,
            pointers = sent.map { TouchPointer(it.id and 0xFF, rect.normalizeX(it.x), rect.normalizeY(it.y)) },
        )
    }
}
