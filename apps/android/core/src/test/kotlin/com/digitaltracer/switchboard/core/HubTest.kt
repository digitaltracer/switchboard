package com.digitaltracer.switchboard.core

import java.util.UUID
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertTrue
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import mockwebserver3.Dispatcher
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.RecordedRequest
import okhttp3.OkHttpClient

/** A scripted hub: answers by operation and records what it was sent. */
class FakeHub(var handler: (op: String, body: String) -> Pair<Int, String>) : Dispatcher() {
    data class Sent(val op: String, val request: RecordedRequest, val body: String)

    val requests = mutableListOf<Sent>()

    override fun dispatch(request: RecordedRequest): MockResponse {
        val op = op(request.method!!, request.url.encodedPath)
        val body = request.body?.utf8().orEmpty()
        val (status, json) = synchronized(this) {
            requests += Sent(op, request, body)
            handler(op, body)
        }
        return MockResponse.Builder().code(status).addHeader("content-type", "application/json").body(json).build()
    }

    fun ops() = synchronized(this) { requests.map { it.op } }

    // The spec's operation ids, by method and path.
    private fun op(method: String, path: String): String {
        val p = path.removePrefix("/app/v1")
        return when {
            method == "POST" && p == "/auth/challenge" -> "token_challenge"
            method == "POST" && p == "/auth/token" -> "issue_token"
            method == "GET" && p == "/projects" -> "list_projects"
            method == "POST" && p == "/sessions" -> "start_session"
            method == "GET" && p.startsWith("/operations/") -> "get_operation"
            method == "POST" && p == "/pair" -> "pair"
            method == "POST" && p.endsWith("/challenge") -> "approval_challenge"
            method == "POST" && p.endsWith("/decision") -> "decide_approval"
            else -> "$method $p"
        }
    }
}

class HubTest {
    private val server = MockWebServer()

    @AfterTest
    fun close() = server.close()

    private fun hub(fake: FakeHub, keys: DeviceKeys = SoftwareKeys(), build: Int = 1, slash: Boolean = false): Hub {
        server.dispatcher = fake
        server.start()
        val url = server.url("/").toString().let { if (slash) it else it.trimEnd('/') }
        return Hub.paired(Pairing(url, "h1", "d1", "Phone"), keys, build, OkHttpClient())
    }

    private fun tokenAnswers(op: String): Pair<Int, String>? = when (op) {
        "token_challenge" -> 200 to """{"nonce":"n1","expires_at":42,"message_version":1,"hub_id":"h1","device_id":"d1"}"""
        "issue_token" -> 200 to """{"access_token":"t-${UUID.randomUUID()}","expires_at":${System.currentTimeMillis() + 900_000}}"""
        else -> null
    }

    private fun json(s: String): JsonObject = Json.parseToJsonElement(s).jsonObject

    @Test
    fun errorsComeBackAsTheHubsCodes() = runTest {
        val fake = FakeHub { op, _ ->
            tokenAnswers(op) ?: (409 to """{"error":{"code":"answer_in_herdr","message":"answer it in Herdr"}}""")
        }
        val hub = hub(fake, build = 7, slash = true)
        assertTrue(!hub.baseUrl.endsWith("/"))
        val e = runCatching { hub.deny("a1", "r1") }.exceptionOrNull()
        assertIs<HubError.Hub>(e)
        assertEquals("answer_in_herdr", e.code)
        assertEquals("answer it in Herdr", e.message)
        val sent = fake.requests.first { it.op == "decide_approval" }
        assertEquals("/app/v1/approvals/a1/decision", sent.request.url.encodedPath)
        assertEquals("7", sent.request.headers["x-switchboard-build"])
        assertTrue(sent.request.headers["Authorization"]!!.startsWith("Bearer t-"))
    }

    @Test
    fun tokensAreSignedWithTheAuthKeyAndRefreshedOnce() = runTest {
        val keys = SoftwareKeys()
        var unauthorizedOnce = true
        val fake = FakeHub { op, _ ->
            tokenAnswers(op) ?: if (unauthorizedOnce) {
                unauthorizedOnce = false
                401 to """{"error":{"code":"invalid_token","message":"expired"}}"""
            } else {
                200 to "[]"
            }
        }
        val hub = hub(fake, keys)
        assertTrue(hub.projects().isEmpty())
        assertEquals(
            listOf("token_challenge", "issue_token", "list_projects", "token_challenge", "issue_token", "list_projects"),
            fake.ops(),
        )
        val issue = json(fake.requests[1].body)
        val der = base64UrlDecode(issue.str("signature"))!!
        val message = SignedMessage.token("h1", "d1", "n1", 42)
        assertTrue(P256.verify(P256.publicKey(keys.publicKey(KeyRole.Auth))!!, message.toByteArray(), der))
    }

    @Test
    fun approvingSignsTheChallengeWithTheApprovalKey() = runTest {
        val keys = SoftwareKeys()
        val fake = FakeHub { op, _ ->
            tokenAnswers(op) ?: when (op) {
                "approval_challenge" ->
                    200 to """{"nonce":"an","expires_at":99,"message_version":1,"hub_id":"h1","device_id":"d1","approval_id":"a1"}"""
                else -> 200 to """{"session_id":"s1","approval_id":"a1","outcome":"allow","delivery":"delivered","replayed":false}"""
            }
        }
        val r = hub(fake, keys).approve("a1", "Approve Bash", "r1")
        assertEquals("allow", r.outcome)
        val body = json(fake.requests.last().body)
        assertEquals("allow", body.str("decision"))
        assertEquals("an", body.str("nonce"))
        val der = base64UrlDecode(body.str("signature"))!!
        val message = SignedMessage.approval("h1", "d1", "a1", "an", 99).toByteArray()
        assertTrue(P256.verify(P256.publicKey(keys.publicKey(KeyRole.Approval))!!, message, der))
        assertTrue(!P256.verify(P256.publicKey(keys.publicKey(KeyRole.Auth))!!, message, der))
    }

    @Test
    fun aLostStartIsLookedUpByItsRequestId() = runTest {
        val fake = FakeHub { op, _ ->
            tokenAnswers(op) ?: when (op) {
                "start_session" -> 504 to """{"error":{"code":"outcome_unknown","message":"lost","operation_id":"r9","session_id":"s9"}}"""
                else -> 200 to """{"request_id":"r9","kind":"session_start","state":"done","session_id":"s9"}"""
            }
        }
        val hub = hub(fake)
        assertEquals(StartState.Unknown("r9"), StartFlow.run(hub, "p", "x", null, null, true, "r9"))
        assertEquals(StartState.Started("s9", alreadyRunning = false), StartFlow.reconcile(hub, "r9"))
    }

    @Test
    fun aRequestNotYetSeenIsGivenTimeBeforeItCountsAsNeverSent() = runTest {
        val fake = FakeHub { op, _ ->
            tokenAnswers(op) ?: (404 to """{"error":{"code":"unknown_operation","message":"no request with this id reached the hub"}}""")
        }
        val hub = hub(fake)
        assertEquals(StartState.Unknown("r1"), StartFlow.reconcile(hub, "r1", attempt = 0))
        assertIs<StartState.Failed>(StartFlow.reconcile(hub, "r1", attempt = 2), "after a few looks it never reached the hub")
    }

    @Test
    fun deletingASessionPostsToItsDeleteRoute() = runTest {
        val fake = FakeHub { op, _ ->
            tokenAnswers(op) ?: (200 to """{"worktree_removed":false,"kept_because":"unmerged","branch":"sb/s1","already_deleted":false}""")
        }
        val r = hub(fake).delete("s1")
        assertEquals("unmerged", r.keptBecause)
        assertEquals("sb/s1", r.branch)
        val sent = fake.requests.last()
        assertEquals("POST", sent.request.method)
        assertEquals("/app/v1/sessions/s1/delete", sent.request.url.encodedPath)
    }

    @Test
    fun requestsMatchWhatShippedBuildsSend() = runTest {
        // `tests/fixtures/app-v1/requests/` is what the hub promises to keep accepting.
        val fake = FakeHub { op, _ ->
            tokenAnswers(op) ?: when (op) {
                "pair" -> 200 to """{"device_id":"d1","fingerprint":"f","hub_id":"h1","state":"pending"}"""
                "start_session" -> 200 to """{"session_id":"s1","label":"l","project_id":"switchboard","machine_id":"mac",
                    "agent":"claude","muted":false,"outcome":"started","operation_id":"o"}"""
                else -> 200 to """{"session_id":"s1","approval_id":"a1","outcome":"deny","replayed":false}"""
            }
        }
        val hub = hub(fake)
        hub.claim("c0de", "Phone", 1)
        hub.start("switchboard", "fix the flaky hub test", null, null, true, "6F9619FF-8B86-D011-B42D-00C04FC964FF")
        hub.deny("a1", "1B4E28BA-2FA1-11D2-883F-0016D3CCA427")
        fun sent(op: String) = json(fake.requests.first { it.op == op }.body)

        assertEquals(Fixtures.json("requests/pair.json").keys, sent("pair").keys)
        assertEquals(Fixtures.json("requests/deny.json"), sent("decide_approval"))
        // iOS sends `notify` too; the shipped fixture predates it.
        assertEquals(Fixtures.json("requests/start.json"), JsonObject(sent("start_session") - "notify"))
    }
}
