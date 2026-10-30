package com.achreftlili.sidecast.video

/** Annex-B elementary stream helpers (pure Kotlin). */
object AnnexB {
    /** NAL unit `[start, end)` in the source array; [start] is the NAL header byte. */
    data class Nal(val start: Int, val end: Int)

    private val START_CODE = byteArrayOf(0, 0, 0, 1)

    /** Splits `data[offset until offset+length]` on 3- or 4-byte start codes. */
    fun nalUnits(data: ByteArray, offset: Int = 0, length: Int = data.size - offset): List<Nal> {
        val end = offset + length
        val result = ArrayList<Nal>(4)
        var nalStart = -1
        var i = offset
        while (i + 2 < end) {
            if (data[i].toInt() == 0 && data[i + 1].toInt() == 0 && data[i + 2].toInt() == 1) {
                if (nalStart >= 0) result += Nal(nalStart, trimTrailingZeros(data, nalStart, i))
                nalStart = i + 3
                i += 3
            } else {
                i++
            }
        }
        if (nalStart in offset until end) result += Nal(nalStart, end)
        return result
    }

    fun nalType(data: ByteArray, nal: Nal, hevc: Boolean): Int {
        val header = data[nal.start].toInt() and 0xFF
        return if (hevc) (header ushr 1) and 0x3F else header and 0x1F
    }

    /** HEVC VPS/SPS/PPS (32..34); H.264 SPS/PPS (7, 8). */
    fun isParameterSet(type: Int, hevc: Boolean): Boolean = if (hevc) type in 32..34 else type == 7 || type == 8

    /**
     * The parameter-set NAL units of an access unit, each re-prefixed with a 4-byte start code,
     * concatenated in stream order; null when the AU carries none. This is exactly what
     * MediaCodec accepts as one BUFFER_FLAG_CODEC_CONFIG buffer (csd-0 style for HEVC, SPS+PPS
     * for H.264).
     */
    fun parameterSets(data: ByteArray, offset: Int, length: Int, hevc: Boolean): ByteArray? {
        val sets = nalUnits(data, offset, length).filter { it.end > it.start && isParameterSet(nalType(data, it, hevc), hevc) }
        if (sets.isEmpty()) return null
        val out = ByteArray(sets.sumOf { START_CODE.size + it.end - it.start })
        var pos = 0
        for (nal in sets) {
            START_CODE.copyInto(out, pos)
            pos += START_CODE.size
            data.copyInto(out, pos, nal.start, nal.end)
            pos += nal.end - nal.start
        }
        return out
    }

    /** A NAL never ends in 0x00, so zeros before a start code belong to the 4-byte form. */
    private fun trimTrailingZeros(data: ByteArray, start: Int, end: Int): Int {
        var e = end
        while (e > start && data[e - 1].toInt() == 0) e--
        return e
    }
}
