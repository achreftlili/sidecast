package com.achreftlili.sidecast.video

import com.achreftlili.sidecast.protocol.hexToBytes
import com.achreftlili.sidecast.protocol.toHex
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AnnexBTest {
    private fun hex(s: String) = hexToBytes(s.replace(" ", ""))

    // HEVC: VPS(32) SPS(33) PPS(34) IDR_W_RADL(19); NAL header = type << 1, layer/tid byte 01.
    private val hevcKeyframe = hex(
        "00 00 00 01 40 01 0c 01" + // VPS
            "00 00 00 01 42 01 01 02" + // SPS
            "00 00 01 44 01 c1 72" + // PPS (3-byte start code)
            "00 00 00 01 26 01 af 09 00 00 03 01", // IDR slice with emulation prevention
    )

    @Test
    fun splitsOnThreeAndFourByteStartCodes() {
        val nals = AnnexB.nalUnits(hevcKeyframe)
        assertEquals(4, nals.size)
        assertEquals(listOf(32, 33, 34, 19), nals.map { AnnexB.nalType(hevcKeyframe, it, hevc = true) })
        // The zero byte of the following 4-byte start code is not part of the PPS.
        val pps = nals[2]
        assertEquals("4401c172", hevcKeyframe.copyOfRange(pps.start, pps.end).toHex())
    }

    @Test
    fun extractsHevcParameterSets() {
        val csd = AnnexB.parameterSets(hevcKeyframe, 0, hevcKeyframe.size, hevc = true)!!
        assertEquals("00000001" + "40010c01" + "00000001" + "42010102" + "00000001" + "4401c172", csd.toHex())
    }

    @Test
    fun extractsH264ParameterSetsFromOffset() {
        val au = hex("ff ff 00 00 00 01 09 f0 00 00 00 01 67 42 c0 1f 00 00 00 01 68 ce 3c 80 00 00 01 65 88 84")
        val csd = AnnexB.parameterSets(au, 2, au.size - 2, hevc = false)!!
        assertEquals("00000001" + "6742c01f" + "00000001" + "68ce3c80", csd.toHex())
    }

    @Test
    fun deltaFrameHasNoParameterSets() {
        val delta = hex("00 00 00 01 02 01 d0 2f 11")
        assertNull(AnnexB.parameterSets(delta, 0, delta.size, hevc = true))
        assertEquals(1, AnnexB.nalType(delta, AnnexB.nalUnits(delta)[0], hevc = true))
    }

    @Test
    fun emptyAndGarbageInput() {
        assertEquals(0, AnnexB.nalUnits(ByteArray(0)).size)
        assertEquals(0, AnnexB.nalUnits(hex("01 02 03 04")).size)
        assertNull(AnnexB.parameterSets(ByteArray(0), 0, 0, hevc = false))
    }
}
