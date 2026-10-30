package com.achreftlili.sidecast.net

import com.achreftlili.sidecast.protocol.MessageCodec
import com.achreftlili.sidecast.protocol.NetworkKind
import com.achreftlili.sidecast.protocol.StreamLimits
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class DataSaverTest {
    private fun json(metered: Boolean, enabled: Boolean) =
        JSONObject(String(MessageCodec.encodePayload(DataSaver.apply(TEST_HELLO, metered, enabled)), Charsets.UTF_8))

    @Test
    fun meteredWithDataSaverAsksForTheCaps() {
        val hello = DataSaver.apply(TEST_HELLO, metered = true, enabled = true)
        assertEquals(NetworkKind.METERED, hello.network)
        assertEquals(StreamLimits(4_000, 30), hello.limits)
        val o = json(metered = true, enabled = true)
        assertEquals("metered", o.getString("network"))
        assertEquals(4000, o.getJSONObject("limits").getInt("maxBitrateKbps"))
        assertEquals(30, o.getJSONObject("limits").getInt("maxFps"))
    }

    @Test
    fun unmeteredHasNoLimits() {
        val hello = DataSaver.apply(TEST_HELLO, metered = false, enabled = true)
        assertEquals(NetworkKind.UNMETERED, hello.network)
        assertNull(hello.limits)
        val o = json(metered = false, enabled = true)
        assertEquals("unmetered", o.getString("network"))
        assertFalse(o.has("limits"))
    }

    @Test
    fun dataSaverOffStillTellsTheTruthAboutTheNetwork() {
        val hello = DataSaver.apply(TEST_HELLO, metered = true, enabled = false)
        assertEquals(NetworkKind.METERED, hello.network) // §5: whether the active network is metered
        assertNull(hello.limits)
        assertFalse(json(metered = true, enabled = false).has("limits"))
    }

    @Test
    fun everythingElseIsUnchanged() {
        val hello = DataSaver.apply(TEST_HELLO, metered = true, enabled = true)
        assertEquals(TEST_HELLO, hello.copy(network = null, limits = null))
    }
}
