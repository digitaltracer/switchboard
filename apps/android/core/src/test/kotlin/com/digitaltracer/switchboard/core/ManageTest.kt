package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.AgentView
import com.digitaltracer.switchboard.api.models.EndedAgent
import com.digitaltracer.switchboard.api.models.Session
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import mockwebserver3.MockWebServer
import okhttp3.OkHttpClient

/** The same cases, in the same words, as the iOS package's `ManageTests`. */
class ManageTest {
    private val json = Json { ignoreUnknownKeys = true }
    private val server = MockWebServer()

    @AfterTest
    fun close() = server.close()

    /** A session as a hub that knows about renames lists it; [extra] is more JSON fields. */
    private fun task(id: String, herdr: String, project: String = "kraft", lifecycle: String = "running", extra: String = ""): Session =
        json.decodeFromString("""
        {"id":"$id","label":"$project-$id","project_id":"$project","machine_id":"mac","machine_status":"ready",
         "agent":"claude","lifecycle":"$lifecycle","agent_status":"idle","hook_failed":false,"revision":1,
         "created_at":0,"updated_at":0,"muted":false,"notifies":true,"pending_approvals":[],"herdr_session":"$herdr"$extra}
        """)

    private fun agent(name: String, extra: String = ""): AgentView =
        json.decodeFromString("""{"machine_id":"mac","name":"$name","herdr_session":"${name.substringBefore('/')}","agent_status":"idle"$extra}""")

    private fun ended(name: String): EndedAgent = json.decodeFromString(
        """{"machine_id":"mac","name":"$name","herdr_session":"${name.substringBefore('/')}","started_at":0,"ended_at":5,"space":"gone"}""",
    )

    private fun spaces(sessions: List<Session> = emptyList(), agents: List<AgentView> = emptyList(), ended: List<EndedAgent> = emptyList()) =
        AgentSpace.group(sessions, agents, ended, since = null)

    @Test
    fun aHerdrAppSpaceCanBeRenamedAndTakeAnAgentWhenItsAgentsAgreeOnItsId() {
        val all = spaces(
            listOf(task("9f3c", "hc", extra = ""","space":"atlas","space_id":"sp1","can_rename":true""")),
            listOf(
                agent("hc/search", ""","space":"atlas","space_id":"sp1","can_rename":true"""),
                agent("hc/scratch", ""","space":"core","can_rename":false"""),
            ),
        )
        val atlas = all.first { it.title == "atlas" }
        assertEquals("sp1", atlas.spaceId)
        assertEquals(Offer.Yes, atlas.rename)
        assertEquals(Offer.Yes, atlas.newAgent)
        assertEquals("kraft", atlas.project, "its one session's project is offered first")
        // Agents outside every space have no space to rename or start into, and nothing to say.
        val outside = all.first { it.key.space == null }
        assertNull(outside.spaceId)
        assertEquals(Offer.Hidden, outside.rename)
        assertEquals(Offer.Hidden, outside.newAgent)
    }

    @Test
    fun twoSpacesWithOneNameAreLeftToTheMac() {
        val all = spaces(
            listOf(
                task("9f3c", "hc", extra = ""","space":"kraft","space_id":"sp1","can_rename":true"""),
                task("0a1b", "hc", project = "atlas", extra = ""","space":"kraft","space_id":"sp2","can_rename":true"""),
            ),
        )
        assertEquals(1, all.size, "the phone shows them as one")
        assertNull(all[0].spaceId)
        assertEquals(Offer.No("Several spaces share this name. Rename one at the Mac."), all[0].rename)
        assertEquals(Offer.No("Several spaces share this name. Start it at the Mac."), all[0].newAgent)
        assertNull(all[0].project, "two projects: none is picked for you")
    }

    @Test
    fun aHerdrTerminalSpaceIsRenamedOnlyWhereItsWorkspaceNameShows() {
        val all = spaces(
            listOf(task("9f3c", "kraft-9f3c", extra = ""","space":"kraft","space_id":"w1","can_rename":true""")),
            listOf(
                agent("main/api", ""","space":"billing","space_id":"w1","can_rename":true"""),
                agent("main/web", ""","space":"atlas","space_id":"w2","can_rename":true"""),
            ),
        )
        val billing = all.first { it.title == "main › billing" }
        assertEquals(Offer.Yes, billing.rename)
        assertEquals("w1", billing.spaceId)
        assertEquals(Offer.No("New agents here start at the Mac."), billing.newAgent)
        // One workspace: the phone shows the Herdr session's name, which Herdr can't change.
        val own = all.first { it.title == "kraft-9f3c" }
        assertEquals(Offer.No("This Herdr session's name is fixed. Rename a workspace at the Mac."), own.rename)
        assertEquals(Offer.No("New agents here start at the Mac."), own.newAgent)
    }

    @Test
    fun anOlderHubOrAnEndedSpaceOffersNothing() {
        val old = spaces(agents = listOf(agent("hc/search", ""","space":"atlas"""")))
        assertEquals(Offer.Hidden, old[0].rename)
        assertEquals(Offer.Hidden, old[0].newAgent)
        val gone = spaces(ended = listOf(ended("hc/old")))
        assertEquals(Offer.Hidden, gone[0].rename)
        assertEquals(Offer.Hidden, gone[0].newAgent)
    }

    @Test
    fun anAgentSaysWhetherItCanBeRenamed() {
        val session = HerdrSession.Agent.Task(task("9f3c", "hc", extra = ""","can_rename":true"""))
        assertEquals(Offer.Yes, session.rename)
        assertTrue(!session.isHerdrAgent)
        assertEquals(Offer.Hidden, HerdrSession.Agent.Task(task("9f3c", "hc")).rename, "an older hub")
        val deleted = HerdrSession.Agent.Task(task("9f3c", "hc", lifecycle = "deleted", extra = ""","can_rename":true"""))
        assertEquals(Offer.Hidden, deleted.rename)

        val own = HerdrSession.Agent.Live(agent("main/api", ""","can_rename":true"""))
        assertEquals(Offer.Yes, own.rename)
        assertTrue(own.isHerdrAgent)
        val fixed = HerdrSession.Agent.Live(agent("hc/review", ""","can_rename":false"""))
        assertEquals(Offer.No("Rename it at the Mac."), fixed.rename)
        assertTrue(!fixed.isHerdrAgent)
        assertEquals(Offer.Hidden, HerdrSession.Agent.Live(agent("hc/review")).rename, "an older hub")
        assertEquals(Offer.Hidden, HerdrSession.Agent.Ended(ended("hc/old")).rename)
    }

    @Test
    fun namesAreCheckedBeforeTheyAreSent() {
        assertNull(Names.problem("login fix"))
        assertEquals("Give it a name.", Names.problem("  "))
        assertNull(Names.problem("x".repeat(40)))
        assertEquals("Keep it to 40 characters.", Names.problem("x".repeat(41)))
        assertEquals("A name can't contain a slash or a line break.", Names.problem("a/b"))
        assertEquals("A name can't contain a slash or a line break.", Names.problem("two\nlines"))
        // Herdr's own rule for an agent in a Herdr terminal.
        assertNull(Names.problem("reviewer-2_a", herdrAgent = true))
        for (bad in listOf("Reviewer", "2fast", "my agent", "é", "a".repeat(33))) {
            assertEquals(Names.HERDR_AGENT_RULE, Names.problem(bad, herdrAgent = true), bad)
        }
        assertEquals("Give it a name.", Names.problem("", herdrAgent = true))
    }

    @Test
    fun aFailedRenameIsSaidInWords() {
        fun hub(code: String) = HubError.Hub(code, "the hub's own words", null, null)
        assertEquals("That name is taken.", Names.failure(hub("name_taken")))
        assertEquals("That machine is offline.", Names.failure(hub("machine_offline")))
        assertEquals("It's no longer there.", Names.failure(hub("not_found")))
        assertEquals("It's no longer there.", Names.failure(hub("unknown_session")))
        assertEquals("The layout changed. Try again.", Names.failure(hub("busy")))
        assertEquals("Update Switchboard on that machine first.", Names.failure(hub("runner_too_old")))
        assertEquals("No answer. Check the name in a moment.", Names.failure(hub("outcome_unknown")))
        assertEquals("the hub's own words", Names.failure(hub("bad_request")))
        assertEquals("Can't reach the hub: timed out", Names.failure(HubError.Unreachable("timed out")))
    }

    @Test
    fun renamesAndAStartIntoASpaceSendWhatTheHubExpects() = runTest {
        val token = { op: String ->
            when (op) {
                "token_challenge" -> 200 to """{"nonce":"n1","expires_at":42,"message_version":1,"hub_id":"h1","device_id":"d1"}"""
                "issue_token" -> 200 to """{"access_token":"t-1","expires_at":${System.currentTimeMillis() + 900_000}}"""
                else -> null
            }
        }
        val fake = FakeHub { op, _ ->
            token(op) ?: when (op) {
                "POST /agents/rename" -> 409 to """{"error":{"code":"name_taken","message":"another agent is already called reviewer"}}"""
                "start_session" -> 200 to """{"session_id":"s1","label":"kraft-s1","project_id":"kraft","machine_id":"mac",
                    "agent":"claude","muted":false,"outcome":"started","operation_id":"r3"}"""
                else -> 200 to """{"name":"billing"}"""
            }
        }
        server.dispatcher = fake
        server.start()
        val hub = Hub.paired(Pairing(server.url("/").toString().trimEnd('/'), "h1", "d1", "Phone"), SoftwareKeys(), 1, OkHttpClient())
        fun body(op: String) = Json.parseToJsonElement(fake.requests.first { it.op == op }.body).jsonObject
        fun kotlinx.serialization.json.JsonObject.text(key: String) = getValue(key).jsonPrimitive.content

        assertEquals("billing", hub.renameSession("s1", "billing"))
        assertEquals("billing", body("POST /sessions/s1/rename").text("name"))

        assertEquals("billing", hub.renameSpace("mac", "hc", "sp1", "billing", "r1"))
        val space = body("POST /spaces/rename")
        assertEquals(listOf("sp1", "hc", "mac", "r1"), listOf("space_id", "herdr_session", "machine_id", "request_id").map { space.text(it) })

        val taken = assertFailsWith<HubError> { hub.renameAgent("mac", "hc/2", "reviewer", "r2") }
        assertEquals("That name is taken.", Names.failure(taken))
        assertEquals("hc/2", body("POST /agents/rename").text("agent"))

        val state = StartFlow.run(hub, "kraft", "fix it", "mac", null, true, "r3", backend = "hc", spaceId = "sp1")
        assertEquals(StartState.Started("s1", alreadyRunning = false), state)
        val start = body("start_session")
        assertEquals("sp1" to "hc", start.text("space_id") to start.text("backend"))
    }
}
