package com.digitaltracer.switchboard.core

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class LiveTest {
    private val w = LiveBoard.Waiting("checkout", "atlas", 900, false)

    @Test
    fun theMoodFollowsTheDesignsStates() {
        assertEquals(LiveMood.Calm, LiveMood.of(LiveBoard(1000, running = listOf("api"), idle = 1), 1000))
        assertEquals(LiveMood.NeedsYou(w, 1), LiveMood.of(LiveBoard(1000, waiting = listOf(w), more = 1), 1000))
        val e = w.copy(error = true)
        assertEquals(LiveMood.Error(e, 0), LiveMood.of(LiveBoard(1000, waiting = listOf(e)), 1000))
        val done = LiveBoard(1000, finished = "kraft", finishedAt = 970)
        assertEquals(LiveMood.Finished("kraft"), LiveMood.of(done, 1000))
        assertEquals(LiveMood.Calm, LiveMood.of(done, 1031))
    }

    @Test
    fun everyStateHasANameWhatHappenedAndTheCounts() {
        val b = LiveBoard(1000, waiting = listOf(w), more = 1, running = listOf("a", "b"), idle = 2)
        val l = LiveLines.of(b, LiveMood.of(b, 1000))
        assertEquals("checkout", l.title)
        assertEquals(900L, l.waitingSince)
        assertEquals(listOf("+1 more", "2", "2"), l.counts.map { it.second })
        assertTrue(l.moreTone)
        val calm = LiveBoard(1000, running = listOf("a", "b", "c"), runningMore = 2, idle = 1)
        val c = LiveLines.of(calm, LiveMood.Calm)
        assertEquals(listOf("5 running", "nothing needs you", "1 idle"), listOf(c.title, c.happened, c.counts.single().second))
    }

    @Test
    fun theSceneMatchesTheIphonesLayout() {
        val b = LiveBoard(1000, waiting = listOf(w), running = listOf("search", "kraft-7c1e", "importer", "api", "web"), idle = 2)
        val layout = LiveLayout.of(b, LiveMood.of(b, 1000))
        assertEquals(4, layout.bodies.count { it.label != null }, "four named planets fit")
        assertEquals(2, layout.bodies.count { it.texture == null }, "idle agents are moons")
        assertEquals(1, layout.bodies.count { it.texture != null && it.label == null }, "the fifth runs unnamed")
        val named = layout.bodies.filter { it.label != null }.map { it.texture }
        assertEquals(named.size, named.toSet().size, "no planet repeats while others are free")
        assertEquals(LiveLayout.texture("checkout"), LiveLayout.texture("checkout"))
        val (x, y) = LiveLayout.point(50.0 to 75.0, 66.0, 35.0)
        assertEquals(106.5, x, 0.1)
        assertEquals(84.3, y, 0.1)
        assertEquals("a-very-long…", LiveLayout.short("a-very-long-agent"))
    }

    @Test
    fun whileWorkingTheCardStartsForWorkAndEndsAfterFifteenQuietMinutes() {
        val now = 10_000_000L
        val quiet = LiveBoard(1, idle = 9)
        val busy = LiveBoard(1, running = listOf("api"))
        val w = LiveShowMode.Working
        assertEquals(LiveShowMode.Step.None, w.decide(quiet, false, null, now))
        assertEquals(LiveShowMode.Step.Show, w.decide(busy, false, null, now))
        assertEquals(LiveShowMode.Step.Show, w.decide(quiet, true, now - 60_000, now))
        assertEquals(LiveShowMode.Step.End, w.decide(quiet, true, now - LiveShowMode.QUIET_END_MS, now))
        assertEquals(LiveShowMode.Step.Show, LiveShowMode.Always.decide(quiet, false, 0, now))
        assertEquals(null, LiveLines.of(quiet, LiveMood.Calm).lead, "no planet beside All quiet")
    }
}
