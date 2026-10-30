package com.achreftlili.sidecast.net

import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.net.ConnectException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class FallbackConnectorTest {
    private fun candidate(host: String) = ConnectCandidate(host, 47800, Route.LAN, 2_500)

    private fun connection(host: String) =
        Connection(ByteArrayInputStream(ByteArray(0)), ByteArrayOutputStream(), ByteArray(32), StreamClient.Endpoint(host, 47800)) {}

    /** Connects, or throws, per host. */
    private class Scripted(private val outcome: () -> Connection) : Connector {
        @Volatile
        var cancelled = false

        override fun connect(): Connection = outcome()

        override fun cancel() {
            cancelled = true
        }
    }

    @Test
    fun firstCandidateThatConnectsWins() {
        val attempts = ArrayList<String>()
        val won = connection("100.101.102.103")
        val connector = FallbackConnector(
            candidates = listOf(candidate("192.168.1.20"), candidate("10.0.0.5"), candidate("100.101.102.103"), candidate("never")),
            connectorFor = { c ->
                Scripted {
                    when (c.host) {
                        "100.101.102.103" -> won
                        "never" -> throw AssertionError("tried past the winner")
                        else -> throw ConnectException("refused")
                    }
                }
            },
            onAttempt = { attempts += it.host },
        )
        assertSame(won, connector.connect())
        assertEquals(listOf("192.168.1.20", "10.0.0.5", "100.101.102.103"), attempts)
    }

    @Test
    fun pinMismatchAtOneAddressDoesNotStopTheOthers() {
        val won = connection("100.101.102.103")
        val connector = FallbackConnector(
            listOf(candidate("192.168.1.20"), candidate("100.101.102.103")),
            { c -> Scripted { if (c.host == "192.168.1.20") throw IdentityChangedException() else won } },
        )
        assertSame(won, connector.connect())
    }

    @Test
    fun pinMismatchIsReportedWhenNothingConnects() {
        val connector = FallbackConnector(
            listOf(candidate("a"), candidate("b"), candidate("c")),
            { c -> Scripted { if (c.host == "b") throw IdentityChangedException() else throw ConnectException(c.host) } },
        )
        try {
            connector.connect()
            fail("connected")
        } catch (_: IdentityChangedException) {
        }
    }

    @Test
    fun lastErrorIsReportedOtherwise() {
        val connector = FallbackConnector(listOf(candidate("a"), candidate("b")), { c -> Scripted { throw ConnectException(c.host) } })
        try {
            connector.connect()
            fail("connected")
        } catch (e: IOException) {
            assertEquals("b", e.message)
        }
        try {
            FallbackConnector(emptyList(), { throw AssertionError("no candidate to build") }).connect()
            fail("connected to nothing")
        } catch (_: IOException) {
        }
    }

    @Test
    fun cancelStopsTheCurrentAttemptAndTheRest() {
        val entered = CountDownLatch(1)
        val release = CountDownLatch(1)
        val tried = ArrayList<String>()
        var first: Scripted? = null
        val connector = FallbackConnector(
            listOf(candidate("slow"), candidate("next")),
            { c ->
                tried += c.host
                Scripted {
                    entered.countDown()
                    release.await(5, TimeUnit.SECONDS)
                    throw ConnectException("cancelled")
                }.also { if (first == null) first = it }
            },
        )
        var error: Throwable? = null
        val t = Thread {
            try {
                connector.connect()
            } catch (e: Throwable) {
                error = e
            }
        }
        t.start()
        assertTrue(entered.await(5, TimeUnit.SECONDS))
        connector.cancel()
        assertTrue("the running attempt was cancelled", first!!.cancelled)
        release.countDown()
        t.join(5_000)
        assertTrue(error is IOException)
        assertEquals(listOf("slow"), tried)
    }
}
