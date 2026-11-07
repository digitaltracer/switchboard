package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.AgentView
import com.digitaltracer.switchboard.api.models.Agents
import com.digitaltracer.switchboard.api.models.Session
import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

class ActivityTest {
    private val json = Json { ignoreUnknownKeys = true }

    private fun session(id: String, machine: String = "mac"): Session = json.decodeFromString("""
        {"id":"$id","label":"kraft-$id","project_id":"kraft","machine_id":"$machine","machine_status":"ready",
         "agent":"claude","lifecycle":"running","agent_status":"working","hook_failed":false,"revision":1,
         "created_at":0,"updated_at":0,"muted":false,"notifies":true,"pending_approvals":[]}
    """)

    private fun live(name: String, machine: String = "mac"): AgentView =
        json.decodeFromString("""{"machine_id":"$machine","name":"$name","agent_status":"working"}""")

    private val withActivity: Agents = json.decodeFromString("""
        {"agents": [], "ended": [],
         "activity": [{"machine_id":"mac","name":"crm/claude","line":"Ran cargo test","at":"2026-11-07T09:00:00Z"},
                      {"machine_id":"mac","name":"crm/w1:p2"},
                      {"machine_id":"mac","name":"sb-9f3c","session_id":"9f3c","line":"Tests pass."}],
         "activity_machines": ["mac"]}
    """)

    @Test
    fun tilesReadTheirLineFromActivity() {
        val lines = TileLines.of(withActivity)!!
        assertEquals(TileLines.Line.Known("Ran cargo test"), lines.line(HerdrSession.Agent.Live(live("crm/claude"))))
        assertEquals(TileLines.Line.Known("Tests pass."), lines.line(HerdrSession.Agent.Task(session("9f3c"))))
        assertEquals(TileLines.Line.Known(null), lines.line(HerdrSession.Agent.Live(live("crm/w1:p2"))), "no line yet means none, not a read")
        assertEquals(TileLines.Line.Unknown, lines.line(HerdrSession.Agent.Live(live("api/codex", machine = "vps"))), "an older runner's machine")
        assertEquals(TileLines.Line.Unknown, lines.line(HerdrSession.Agent.Task(session("0000", machine = "vps"))))
    }

    @Test
    fun anOlderHubGivesNoActivity() {
        assertNull(TileLines.of(json.decodeFromString("""{"agents":[],"ended":[]}""")))
    }

    @Test
    fun aTileOutsideActivityReadsItsConversationAtMostEvery30Seconds() {
        val pacer = FallbackPacer()
        assertTrue(pacer.due("a", 1_000))
        assertFalse(pacer.due("a", 6_000))
        assertTrue(pacer.due("b", 6_000), "each tile on its own")
        assertFalse(pacer.due("a", 30_999))
        assertTrue(pacer.due("a", 31_000))
    }
}
