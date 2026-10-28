package com.digitaltracer.switchboard.core

import java.net.URLEncoder
import java.security.SecureRandom
import java.security.interfaces.ECPublicKey
import java.util.Base64
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertContains
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlinx.coroutines.test.runTest
import mockwebserver3.Dispatcher
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.RecordedRequest
import mockwebserver3.SocketEffect
import okhttp3.OkHttpClient

class RelayTest {
    @Test
    fun theHubsRustEnvelopesInteroperate() {
        val v = Fixtures.relayVector
        val hub = P256.publicKey(base64UrlDecode(v.str("hub_kx_public"))!!)!!
        val eph = P256.privateKey(hexBytes(v.str("phone_secret_hex")))
        val request = v.obj("swift_request")
        val ephPublic = P256.publicKey(base64UrlDecode(request.str("epk"))!!)!!
        val keys = RelaySeal.keys(RelaySeal.agree(eph, hub), ephPublic, hub)

        // The same request the fixture's Swift half sealed.
        val plain = """{"body":"","headers":[["x-switchboard-build","1"]],"method":"GET","path":"/app/v1/meta","ts":42}"""
        assertEquals(request.str("ct"), RelaySeal.seal(plain.toByteArray(), keys.request).base64Url)

        // The hub's Rust reply opens with this phone's reply key.
        val reply = v.obj("rust_response")
        val opened = RelaySeal.open(base64UrlDecode(reply.str("ct"))!!, keys.reply, base64UrlDecode(reply.str("n"))!!)
        val answer = RelaySeal.json.decodeFromString<RelaySeal.PlainResponse>(opened.decodeToString())
        assertEquals(200, answer.status)
        assertEquals("""{"ok":true}""", Base64.getDecoder().decode(answer.body).decodeToString())

        // The relay id is the hub's, from both keys.
        val route = RelayRoute.of("https://adarshnb.com/relay/", v.str("hub_kx_public"), v.str("sign_public"))
        assertEquals(v.str("relay_id"), route?.relayId)
        assertEquals("https://adarshnb.com/relay", route?.relayUrl)
    }

    @Test
    fun relayPairingLinksCarryTheHubsKeys() {
        val v = Fixtures.relayVector
        fun link(q: List<Pair<String, String>>) =
            "switchboard://pair?" + q.joinToString("&") { (k, x) -> "$k=${URLEncoder.encode(x, Charsets.UTF_8)}" }
        val all = listOf(
            "relay" to "https://adarshnb.com/relay", "k" to v.str("hub_kx_public"),
            "s" to v.str("sign_public"), "code" to "c0de", "hub" to "h1",
        )
        val parsed = PairingLink.parse(link(all))
        assertEquals(v.str("relay_id"), parsed?.relay?.relayId)
        assertEquals("https://adarshnb.com/relay", parsed?.hubUrl)
        assertNull(PairingLink.parse(link(all.filter { it.first != "k" })), "a relay link without the hub's key is refused")
    }

    private val server = MockWebServer()

    @AfterTest
    fun close() = server.close()

    @Test
    fun requestsGoThroughTheRelayEncryptedAndAnOfflineHubIsUnreachable() = runTest {
        val relay = MockRelay()
        server.dispatcher = relay
        server.start()
        val route = RelayRoute.of(
            server.url("/relay").toString(),
            P256.sec1(relay.hub.public as ECPublicKey).base64Url,
            SoftwareKeys().publicKey(KeyRole.Auth).base64Url,
        )!!
        val hub = Hub.paired(Pairing(route.relayUrl, "h1", "d1", "Phone", route), SoftwareKeys(), 3, OkHttpClient())

        assertEquals("h1", hub.meta().hubId)
        assertEquals("/relay/h/${route.relayId}", relay.paths.last())
        val seen = relay.seen.last()
        assertEquals("GET", seen.method)
        assertEquals("/app/v1/meta", seen.path)
        assertContains(seen.headers, listOf("x-switchboard-build", "3"))

        relay.status = 503
        assertIs<HubError.Unreachable>(runCatching { hub.meta() }.exceptionOrNull(), "an offline hub is unreachable")

        relay.status = 502
        relay.errorPage = "<html>502 Bad Gateway</html>"
        assertIs<HubError.Unreachable>(runCatching { hub.meta() }.exceptionOrNull(), "a relay that's down is unreachable")
    }

    @Test
    fun aDroppedConnectionIsRetriedOnceSealedAfresh() = runTest {
        val relay = MockRelay()
        server.dispatcher = relay
        server.start()
        val route = RelayRoute.of(
            server.url("/relay").toString(),
            P256.sec1(relay.hub.public as ECPublicKey).base64Url,
            SoftwareKeys().publicKey(KeyRole.Auth).base64Url,
        )!!
        val hub = Hub.paired(Pairing(route.relayUrl, "h1", "d1", "Phone", route), SoftwareKeys(), 3, OkHttpClient())

        relay.drops = 1
        assertEquals("h1", hub.meta().hubId)
        assertEquals(2, relay.epks.size)
        assertEquals(2, relay.epks.toSet().size, "the hub refuses a key it has seen, so the retry needs its own")

        relay.drops = 2
        assertNull(runCatching { hub.meta() }.getOrNull(), "only one retry")
        assertEquals(4, relay.epks.size)
    }

    /** Plays relay and hub at once: opens the phone's envelope with the hub key and answers it. */
    private class MockRelay : Dispatcher() {
        val hub = RelaySeal.ephemeral()
        @Volatile var status = 200
        /** A non-JSON error page, as nginx sends when the relay is down. */
        @Volatile var errorPage: String? = null
        /** Requests to answer by closing the connection instead. */
        @Volatile var drops = 0
        val seen = mutableListOf<RelaySeal.PlainRequest>()
        val paths = mutableListOf<String>()
        val epks = mutableListOf<String>()

        override fun dispatch(request: RecordedRequest): MockResponse {
            paths += request.url.encodedPath
            if (status != 200) {
                return MockResponse.Builder().code(status)
                    .body(errorPage ?: """{"error":{"code":"hub_offline","message":"x"}}""").build()
            }
            val env = RelaySeal.json.decodeFromString<RelaySeal.Envelope>(request.body!!.utf8())
            epks += env.epk!!
            if (drops > 0) {
                drops--
                return MockResponse.Builder().onResponseStart(SocketEffect.CloseSocket()).build()
            }
            val epk = P256.publicKey(base64UrlDecode(env.epk!!)!!)!!
            val keys = RelaySeal.keys(RelaySeal.agree(hub.private, epk), epk, hub.public)
            val plain = RelaySeal.open(base64UrlDecode(env.ct)!!, keys.request)
            seen += RelaySeal.json.decodeFromString<RelaySeal.PlainRequest>(plain.decodeToString())
            val meta = """{"api_version":1,"hub_id":"h1","min_client_build":1,"message_version":1,"max_wait_ms":25000}"""
            val answer = RelaySeal.PlainResponse(
                200, listOf(listOf("content-type", "application/json")), Base64.getEncoder().encodeToString(meta.toByteArray()),
            )
            val nonce = ByteArray(12).also(SecureRandom()::nextBytes)
            val ct = RelaySeal.seal(RelaySeal.json.encodeToString(answer).toByteArray(), keys.reply, nonce)
            return MockResponse.Builder().code(200).addHeader("content-type", "application/json")
                .body(RelaySeal.json.encodeToString(RelaySeal.Envelope(RelaySeal.VERSION, n = nonce.base64Url, ct = ct.base64Url)))
                .build()
        }
    }
}
