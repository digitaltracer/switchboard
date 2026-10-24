package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.infrastructure.Serializer
import com.digitaltracer.switchboard.api.models.Agents
import com.digitaltracer.switchboard.api.models.Changes
import com.digitaltracer.switchboard.api.models.MessagePreview
import com.digitaltracer.switchboard.api.models.Session
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class RoutingTest {
    private inline fun <reified T> decode(json: String): T = Serializer.kotlinxSerializationJson.decodeFromString(json)

    private fun session(id: String, project: String, lifecycle: String = "running"): Session = decode(
        """
        {"id":"$id","label":"$project-$id","project_id":"$project","machine_id":"mac","machine_status":"ready",
         "agent":"claude","lifecycle":"$lifecycle","agent_status":"working","hook_failed":false,"revision":1,
         "created_at":0,"updated_at":0,"muted":false,"notifies":true,"pending_approvals":[]}
        """,
    )

    @Test
    fun aStartOffersTheProjectsRunningSessionsToo() {
        val preview: MessagePreview = decode(
            """{"outcome":"start","message":"Starts a new session in kraft on mac.","project_id":"kraft","machine_id":"mac","options":[]}""",
        )
        val sessions = listOf(session("9f3c", "kraft"), session("1a2b", "switchboard"), session("0000", "kraft", "stopped"))
        val r = RouteOptions.build(preview, sessions, null)
        assertEquals(listOf("New task in kraft", "Continue kraft-9f3c"), r.options.map { it.title })
        assertEquals("start:kraft", r.suggested)
        assertEquals(RouteOption.Action.AsPreviewed, r.options[0].action)
        assertEquals(RouteOption.Action.Send(sessionTarget(sessions[0])), r.options[1].action)
    }

    @Test
    fun aSendOffersANewTaskInTheSameProject() {
        val preview: MessagePreview = decode(
            """{"outcome":"send","message":"Sends to kraft-9f3c.","target":{"kind":"session","session_id":"9f3c","machine_id":"mac","label":"kraft-9f3c"},"options":[]}""",
        )
        val r = RouteOptions.build(preview, listOf(session("9f3c", "kraft")), null)
        assertEquals(listOf("Continue kraft-9f3c", "New task in kraft"), r.options.map { it.title })
        assertEquals(RouteOption.Action.Start("kraft"), r.options[1].action)
        assertEquals("Send to kraft-9f3c", r.options[0].verb)
    }

    @Test
    fun whenTheHubWouldAskNothingIsSuggestedAndTheContextComesFirst() {
        val preview: MessagePreview = decode(
            """{"outcome":"choose_project","message":"Which project?","options":[{"label":"kraft","project_id":"kraft"},{"label":"niti","project_id":"niti"}]}""",
        )
        val r = RouteOptions.build(preview, emptyList(), agentTarget("mac", "mono1/claude"))
        assertNull(r.suggested)
        assertEquals(listOf("Continue mono1/claude", "New task in kraft", "New task in niti"), r.options.map { it.title })
    }

    @Test
    fun changesDecodeWithoutCountsForNewFiles() {
        val c: Changes = decode("""{"files":[{"path":"a.rs","added":3,"deleted":1},{"path":"new.rs"}],"truncated":false}""")
        assertEquals(listOf<Long?>(3, null), c.files.map { it.added })
    }

    @Test
    fun anAgentWithoutANameShowsWhatItsTerminalSays() {
        val agents: Agents = decode(
            """
            {"agents":[{"machine_id":"mac","name":"mono1/claude","agent_status":"idle","title":"Ignored"},
                       {"machine_id":"mac","name":"mono1/w1:p2","agent_status":"idle","title":"Fix the flaky test"},
                       {"machine_id":"mac","name":"mono1/w1:p3","agent_status":"idle"}]}
            """,
        )
        assertEquals(listOf("mono1/claude", "Fix the flaky test", "mono1/w1:p3"), agents.agents.map { it.displayName })
    }

    @Test
    fun newStatusValuesAndFieldsDecode() {
        // Additive hub changes must never break an installed app.
        val s: Session = decode(
            """
            {"id":"1","label":"l","project_id":"p","machine_id":"m","machine_status":"brand_new","agent":"claude",
             "lifecycle":"hibernating","agent_status":"x","hook_failed":false,"revision":1,"created_at":0,"updated_at":0,
             "muted":false,"notifies":true,"pending_approvals":[],"some_future_field":{"a":1}}
            """,
        )
        assertEquals("hibernating", s.lifecycle)
    }
}
