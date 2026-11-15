package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.AgentView
import com.digitaltracer.switchboard.api.models.EndedAgent
import com.digitaltracer.switchboard.api.models.Session
import kotlinx.serialization.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class SessionsTest {
    // Its own decoder: the shared one may only be configured once, by the hub client.
    private val json = Json { ignoreUnknownKeys = true }

    private fun task(id: String, herdr: String, lifecycle: String = "running", updated: Long = 0): Session = json.decodeFromString("""
        {"id":"$id","label":"kraft-$id","project_id":"kraft","machine_id":"mac","machine_status":"ready",
         "agent":"claude","lifecycle":"$lifecycle","agent_status":"idle","hook_failed":false,"revision":1,
         "created_at":0,"updated_at":$updated,"muted":false,"notifies":true,"pending_approvals":[],"herdr_session":"$herdr"}
    """)

    private fun agent(name: String, status: String = "idle", title: String? = null, space: String? = null): AgentView {
        val t = title?.let { ""","title":"$it"""" } ?: ""
        val s = space?.let { ""","space":"$it"""" } ?: ""
        return json.decodeFromString("""{"machine_id":"mac","name":"$name","herdr_session":"${name.substringBefore('/')}","agent_status":"$status"$t$s}""")
    }

    private fun ended(name: String, at: Long, space: String? = null): EndedAgent {
        val s = space?.let { ""","space":"$it"""" } ?: ""
        return json.decodeFromString(
            """{"machine_id":"mac","name":"$name","herdr_session":"${name.substringBefore('/')}","started_at":0,"ended_at":$at$s}""",
        )
    }

    @Test
    fun theHerdrAppIsOneSessionWithItsSpacesKeptApart() {
        val groups = HerdrSession.group(
            sessions = emptyList(),
            agents = listOf(
                agent("hc/orchestrator", "working", space = "switchboard"),
                agent("hc/orchestrator-t9", space = "udp-backend"),
                agent("hc/reviewer", space = "udp-backend"),
                agent("hc/scratch", space = "core"),
                agent("crm/orchestrator", space = "Imports"),
                agent("crm/w1:p2", title = "Fix the import", space = "1"),
            ),
            ended = listOf(ended("hc/old", 300, "switchboard"), ended("hc/older", 200)),
            since = null,
        )
        assertEquals(listOf("crm", "Herdr App"), groups.map { it.title })
        val app = groups[1]
        assertTrue(app.isHerdrApp && app.key.name == "hc")
        assertEquals(listOf("switchboard", "udp-backend", null), app.spaces.map { it.name })
        assertEquals(listOf("orchestrator", "old"), app.spaces[0].agents.map { it.nameInSpace })
        assertEquals(listOf("orchestrator-t9", "reviewer"), app.spaces[1].live.map { it.nameInSpace })
        // Outside every space, and one that ended before spaces were recorded.
        assertEquals(listOf("scratch", "older"), app.spaces[2].agents.map { it.nameInSpace })
        assertEquals("switchboard › orchestrator", app.live[0].shortName)
        // A Herdr session is split the same way; its numbered default workspace is no name.
        assertEquals(listOf("Imports", null), groups[0].spaces.map { it.name })
        assertEquals(listOf("Fix the import"), groups[0].spaces[1].live.map { it.nameInSpace })
    }

    @Test
    fun agentsGroupBySessionWithEndedOnesLast() {
        val groups = HerdrSession.group(
            sessions = listOf(task("9f3c", "kraft-9f3c"), task("0000", "kraft-0000", "exited", 500)),
            agents = listOf(agent("crm/orchestrator", "working"), agent("crm/w1:p2", title = "Fix the import")),
            ended = listOf(ended("crm/reviewer", 300), ended("crm/old", 50), ended("mono1/w1:p1", 400)),
            since = 100,
        )
        assertEquals(listOf("crm", "kraft-9f3c", "kraft-0000", "mono1"), groups.map { it.key.name })
        val crm = groups[0]
        assertEquals(listOf("orchestrator", "Fix the import", "reviewer"), crm.agents.map { it.shortName }, "ended before `since` is left out")
        assertEquals("Fix the import", crm.live[1].title)
        assertEquals(listOf("ended"), crm.ended.map { it.agentStatus })
        assertTrue(groups[2].isEnded && groups[3].isEnded)
        assertEquals("kraft-9f3c", groups[1].live.first().shortName)
    }

    @Test
    fun aMachineKeepsOnlyItsOwnAndNullSinceKeepsEveryEnded() {
        val stopped = listOf(task("0000", "kraft-0000", "stopped", 1))
        assertTrue(HerdrSession.group(stopped, emptyList(), emptyList(), machine = "vps", since = null).isEmpty())
        val all = HerdrSession.group(stopped, emptyList(), emptyList(), since = null)
        assertTrue(all.size == 1 && all[0].isEnded)
    }
}
