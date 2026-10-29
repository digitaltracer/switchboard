package com.digitaltracer.switchboard.core

import java.util.UUID
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runTest
import mockwebserver3.MockWebServer
import okhttp3.OkHttpClient

class FollowTest {
    private val server = MockWebServer()

    @AfterTest
    fun close() = server.close()

    private fun following(): Pair<FakeHub, Hub> {
        val fake = FakeHub { op, _ ->
            when (op) {
                "token_challenge" -> 200 to """{"nonce":"n1","expires_at":42,"message_version":1,"hub_id":"h1","device_id":"d1"}"""
                "issue_token" -> 200 to """{"access_token":"t-${UUID.randomUUID()}","expires_at":${System.currentTimeMillis() + 900_000}}"""
                else -> 200 to if (op.endsWith("/transcript")) {
                    """{"items":[],"start":10,"end":10}"""
                } else {
                    """{"text":"> _","truncated":false}"""
                }
            }
        }
        server.dispatcher = fake
        server.start()
        return fake to Hub.paired(Pairing(server.url("/").toString().trimEnd('/'), "h1", "d1", "Phone"), SoftwareKeys(), 1, OkHttpClient())
    }

    private fun asked(fake: FakeHub, suffix: String) =
        fake.requests.filter { it.op.endsWith(suffix) }.map { it.request.url.query.orEmpty() }

    @Test
    fun onlyFollowingAsksTheHubToWaitAndOnlyAfterTheFirstLoad() = runTest {
        val (fake, hub) = following()
        for (source in listOf(ConversationSource.Session("s1"), ConversationSource.Agent("mac", "mono1/claude"))) {
            Follow.page(hub, source, null, null, wait = true)
            Follow.page(hub, source, 10, null, wait = true)
            Follow.page(hub, source, null, 10, wait = true)
            // A refresh someone is waiting on, such as after an approval.
            Follow.page(hub, source, 10, null)
        }
        val queries = asked(fake, "/transcript")
        assertEquals(8, queries.size)
        assertEquals(2, queries.count { "wait_ms=20000" in it }, "$queries")
        assertTrue(queries.none { "wait_ms" in it && "after=10" !in it }, "only continuing reads wait")
    }

    @Test
    fun theScreenIsReadVisibleOnly() = runTest {
        val (fake, hub) = following()
        Follow.screen(hub, ConversationSource.Session("s1"))
        Follow.screen(hub, ConversationSource.Agent("mac", "mono1/claude"))
        val queries = asked(fake, "/output")
        assertEquals(2, queries.size)
        assertTrue(queries.all { "visible=true" in it }, "$queries")
    }

    /** Runs [rounds] rounds in virtual time; each takes the time and has the outcome [round] gives. */
    private suspend fun TestScope.starts(rounds: Int, going: () -> Boolean = { true }, round: (Int) -> Pair<Long, Boolean>): List<Long> {
        val starts = mutableListOf<Long>()
        var n = 0
        Follow.repeating(going = { n < rounds && going() }, now = { testScheduler.currentTime }) {
            starts += testScheduler.currentTime
            val (took, ok) = round(n++)
            delay(took)
            ok
        }
        return starts
    }

    @Test
    fun aHubThatDoesntWaitIsAskedNoMoreThanOnceASecond() = runTest {
        assertEquals(listOf(0L, 1_000, 2_000, 3_000), starts(4) { 30L to true })
    }

    @Test
    fun aHubThatWaitsIsAskedAgainAsSoonAsItAnswers() = runTest {
        assertEquals(listOf(0L, 20_000, 21_000), starts(3) { n -> (if (n == 1) 400L else 20_000L) to true })
    }

    @Test
    fun aFailedRequestBacksOff() = runTest {
        assertEquals(listOf(0L, 3_000, 4_000), starts(3) { n -> 100L to (n != 0) })
    }

    @Test
    fun nothingIsAskedOnceTheAppLeavesTheForeground() = runTest {
        var foreground = true
        val starts = starts(10, going = { foreground }) { n ->
            if (n == 1) foreground = false
            20_000L to true
        }
        assertEquals(2, starts.size)
    }

    /** Counts screen reads and how many ran at once. */
    private class Reads {
        var count = 0
        var inFlight = 0
        var most = 0
        suspend fun read(): Boolean {
            count++
            inFlight++
            most = maxOf(most, inFlight)
            delay(20)
            inFlight--
            return true
        }
    }

    @Test
    fun aHiddenScreenIsNotReadUntilItIsShown() = runTest {
        val (bell, reads) = Bell() to Reads()
        var shown = false
        val loop = launch { Follow.screens(bell, { shown }, now = { testScheduler.currentTime }) { reads.read() } }
        delay(60_000)
        assertEquals(0, reads.count, "nothing wakes it while the conversation is shown")
        shown = true
        bell.ring()
        delay(200)
        assertEquals(1, reads.count, "opening the Screen tab reads it at once")
        loop.cancel()
    }

    @Test
    fun openingTheScreenBringsTheNextReadForwardWithoutAnotherAlongside() = runTest {
        val (bell, reads) = Bell() to Reads()
        val loop = launch {
            Follow.screens(bell, { true }, everyMs = 30_000, now = { testScheduler.currentTime }) { reads.read() }
        }
        delay(5)
        bell.ring()
        bell.ring()
        delay(200)
        assertEquals(2, reads.count, "a ring during a read starts one more after it, not one beside it")
        assertEquals(1, reads.most)
        loop.cancel()
    }
}
