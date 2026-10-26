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

    private fun agent(name: String, status: String = "idle", title: String? = null): AgentView {
        val t = title?.let { ""","title":"$it"""" } ?: ""
        return json.decodeFromString("""{"machine_id":"mac","name":"$name","herdr_session":"${name.substringBefore('/')}","agent_status":"$status"$t}""")
    }

    private fun ended(name: String, at: Long): EndedAgent = json.decodeFromString(
        """{"machine_id":"mac","name":"$name","herdr_session":"${name.substringBefore('/')}","started_at":0,"ended_at":$at}""",
    )

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
