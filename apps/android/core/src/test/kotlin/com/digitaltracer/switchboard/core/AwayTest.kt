package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.AwayAgentView
import com.digitaltracer.switchboard.api.models.AwayFactView
import kotlin.test.Test
import kotlin.test.assertEquals

class AwayTest {
    private fun agent(state: String?, facts: List<Pair<Long, String>>, reply: String? = null) =
        AwayAgentView(machineId = "mac", agent = "checkout", facts = facts.map { AwayFactView(it.first, it.second, it.second) }, state = state, lastReply = reply)

    @Test
    fun eachAgentGetsOneLineSayingWhatItIsDoingOrDid() {
        val now = 10_000_000L
        assertEquals("Waiting 22 min: \"asks which provider to keep\"", AwayDigest.line(agent("waiting", listOf(now - 22 * 60_000 to "waiting"), "asks which provider to keep"), now))
        assertEquals("Finished: \"Added backoff.\"", AwayDigest.line(agent("idle", listOf(1L to "working", 2L to "finished"), "Added backoff."), now))
        assertEquals("Stopped with an error", AwayDigest.line(agent("error", listOf(1L to "error")), now))
        assertEquals("Still working · you allowed 2 commands meanwhile", AwayDigest.line(agent("working", listOf(1L to "asked", 2L to "allowed", 3L to "asked", 4L to "allowed")), now))
        assertEquals("1 h 40 m", AwayDigest.duration(100 * 60_000))
    }

    @Test
    fun consecutiveTurnsTellAShortStory() {
        fun f(at: Long, kind: String) = AwayFactView(at, kind, kind)
        val story = AwayDigest.timeline(listOf(f(1, "started"), f(2, "asked"), f(3, "denied"), f(4, "finished"), f(5, "working"), f(6, "finished"), f(7, "working"), f(8, "finished")))
        assertEquals(listOf("started", "asked", "denied", "finished"), story.map { it.kind })
        assertEquals("Finished 3 turns", story.last().text)
        assertEquals(listOf("Finished its turn"), AwayDigest.timeline(listOf(f(1, "working"), f(2, "finished"))).map { it.text })
        assertEquals(listOf("finished", "asked", "working"), AwayDigest.timeline(listOf(f(1, "finished"), f(2, "asked"), f(3, "working"))).map { it.kind })
        val long = AwayDigest.timeline((0..8).map { f(it.toLong(), if (it % 2 == 0) "asked" else "allowed") })
        assertEquals(AwayDigest.MAX_ROWS, long.size)
        assertEquals(8L, long.last().at)
    }
}
