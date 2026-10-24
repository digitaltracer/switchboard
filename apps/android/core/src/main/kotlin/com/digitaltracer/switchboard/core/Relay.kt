package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.ApiError
import java.io.IOException
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.PrivateKey
import java.security.PublicKey
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.util.Base64
import java.util.concurrent.TimeUnit
import javax.crypto.Cipher
import javax.crypto.KeyAgreement
import javax.crypto.Mac
import javax.crypto.spec.IvParameterSpec
import javax.crypto.spec.SecretKeySpec
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import okhttp3.Headers
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.Interceptor
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.MediaType.Companion.toMediaTypeOrNull
import okhttp3.OkHttpClient
import okhttp3.Protocol
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.ResponseBody.Companion.toResponseBody
import okio.Buffer

/**
 * How a paired phone reaches a hub through a relay. The relay id is a hash of the hub's two
 * public keys, so the relay can't send this phone's traffic to a hub of its own choosing.
 */
@Serializable
data class RelayRoute(
    val relayUrl: String,
    /** The hub's key-agreement key, SEC1 uncompressed, base64url. */
    val hubKey: String,
    val signKey: String,
    val relayId: String,
) {
    companion object {
        fun of(relayUrl: String, hubKey: String, signKey: String): RelayRoute? {
            val kx = base64UrlDecode(hubKey) ?: return null
            val sign = base64UrlDecode(signKey) ?: return null
            P256.publicKey(kx) ?: return null
            return RelayRoute(relayUrl.trimEnd('/'), hubKey, signKey, sha256(kx + sign).hex().take(32))
        }
    }
}

/**
 * End-to-end encryption to the hub, as `src/relay/seal.rs` does it: ECDH with a one-off key,
 * HKDF-SHA256 into a request key and a reply key, ChaCha20-Poly1305. Requests use a zero nonce
 * under their single-use key; each reply carries its own random nonce.
 */
internal object RelaySeal {
    const val VERSION = 2

    val json = Json {
        explicitNulls = false
        ignoreUnknownKeys = true
    }

    @Serializable
    data class Envelope(val v: Int, val epk: String? = null, val n: String? = null, val ct: String)

    @Serializable
    data class PlainRequest(
        val ts: Long,
        val method: String,
        val path: String,
        val headers: List<List<String>>,
        val body: String,
    )

    @Serializable
    data class PlainResponse(val status: Int, val headers: List<List<String>>, val body: String)

    class Keys(val request: ByteArray, val reply: ByteArray)

    fun ephemeral(): KeyPair =
        KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()

    /** The ECDH secret, the same from either side: phone's one-off key with the hub's, or the reverse. */
    fun agree(own: PrivateKey, peer: PublicKey): ByteArray = KeyAgreement.getInstance("ECDH").run {
        init(own)
        doPhase(peer, true)
        generateSecret()
    }

    fun keys(shared: ByteArray, phone: PublicKey, hub: PublicKey): Keys {
        val salt = P256.sec1(phone as ECPublicKey) + P256.sec1(hub as ECPublicKey)
        val prk = hmac(salt, shared)
        // One HKDF-Expand block is the 32 bytes each key needs.
        fun derive(info: String) = hmac(prk, info.toByteArray() + byteArrayOf(1))
        return Keys(derive("switchboard-relay-v1 request"), derive("switchboard-relay-v1 reply"))
    }

    fun seal(plain: ByteArray, key: ByteArray, nonce: ByteArray = ByteArray(12)): ByteArray =
        chacha(Cipher.ENCRYPT_MODE, key, nonce).doFinal(plain)

    fun open(ct: ByteArray, key: ByteArray, nonce: ByteArray = ByteArray(12)): ByteArray {
        if (ct.size < 16) throw RelayFailure(HubError.Unexpected("The hub's answer is too short."))
        return chacha(Cipher.DECRYPT_MODE, key, nonce).doFinal(ct)
    }

    private fun hmac(key: ByteArray, data: ByteArray): ByteArray =
        Mac.getInstance("HmacSHA256").run {
            init(SecretKeySpec(key, "HmacSHA256"))
            doFinal(data)
        }

    // The JDK and Android's Conscrypt name the same cipher differently.
    private fun chacha(mode: Int, key: ByteArray, nonce: ByteArray): Cipher {
        val cipher = try {
            Cipher.getInstance("ChaCha20-Poly1305")
        } catch (_: java.security.GeneralSecurityException) {
            Cipher.getInstance("ChaCha20/Poly1305/NoPadding")
        }
        cipher.init(mode, SecretKeySpec(key, "ChaCha20"), IvParameterSpec(nonce))
        return cipher
    }
}

/** A relay answer that already says what went wrong; OkHttp only carries IOExceptions. */
class RelayFailure(val error: HubError) : IOException(error.message)

/**
 * Sends each API request through the relay, encrypted to the hub. The generated client and
 * everything above it don't know the difference. It answers instead of calling `proceed`, so it
 * must be the last interceptor.
 */
class RelayInterceptor(private val route: RelayRoute, private val http: OkHttpClient) : Interceptor {
    private val base = route.relayUrl.toHttpUrl()

    override fun intercept(chain: Interceptor.Chain): Response {
        val request = chain.request()
        val hub = P256.publicKey(base64UrlDecode(route.hubKey) ?: ByteArray(0))
            ?: throw RelayFailure(HubError.Unexpected("Bad hub key."))
        val body = request.body?.let { b -> Buffer().also { b.writeTo(it) }.readByteArray() } ?: ByteArray(0)
        val headers = request.headers.map { listOf(it.first, it.second) }.toMutableList()
        val type = request.body?.contentType()
        if (type != null && headers.none { it[0].equals("content-type", ignoreCase = true) }) {
            headers += listOf("content-type", type.toString())
        }
        val plain = RelaySeal.PlainRequest(
            ts = System.currentTimeMillis(),
            method = request.method,
            path = request.url.encodedPath.removePrefix(base.encodedPath.trimEnd('/')).ifEmpty { "/" } +
                (request.url.encodedQuery?.let { "?$it" } ?: ""),
            headers = headers,
            body = Base64.getEncoder().encodeToString(body),
        )
        val eph = RelaySeal.ephemeral()
        val keys = RelaySeal.keys(RelaySeal.agree(eph.private, hub), eph.public, hub)
        val envelope = RelaySeal.Envelope(
            v = RelaySeal.VERSION,
            epk = P256.sec1(eph.public as ECPublicKey).base64Url,
            ct = RelaySeal.seal(RelaySeal.json.encodeToString(plain).toByteArray(), keys.request).base64Url,
        )
        val post = Request.Builder()
            .url("${route.relayUrl}/h/${route.relayId}")
            .post(RelaySeal.json.encodeToString(envelope).toRequestBody(JSON))
            .build()
        val (status, data) = http.newCall(post).execute().use { it.code to it.body.bytes() }
        if (status != 200) {
            val decoded = runCatching { RelaySeal.json.decodeFromString<ApiError>(data.decodeToString()) }.getOrNull()
            val code = decoded?.error?.code ?: "relay_$status"
            // An offline or slow hub, or a relay that's down, is "can't reach", so a start goes to
            // reconciling, not failed.
            if (code == "hub_offline" || code == "hub_timeout") {
                throw RelayFailure(HubError.Unreachable(
                    if (code == "hub_offline") "Your hub isn't connected to the relay." else "Your hub didn't answer in time.",
                ))
            }
            if (decoded == null && status >= 500) throw RelayFailure(HubError.Unreachable("The relay isn't answering."))
            throw RelayFailure(HubError.Hub(code, decoded?.error?.message ?: "The relay answered $status.", null, null))
        }
        val reply = runCatching { RelaySeal.json.decodeFromString<RelaySeal.Envelope>(data.decodeToString()) }.getOrNull()
        val ct = reply?.ct?.let(::base64UrlDecode)
        val nonce = reply?.n?.let(::base64UrlDecode)
        if (reply?.v != RelaySeal.VERSION || ct == null || nonce?.size != 12) {
            throw RelayFailure(HubError.Unexpected("Unexpected relay answer."))
        }
        val answer = try {
            RelaySeal.json.decodeFromString<RelaySeal.PlainResponse>(RelaySeal.open(ct, keys.reply, nonce).decodeToString())
        } catch (e: java.security.GeneralSecurityException) {
            throw RelayFailure(HubError.Unexpected("The relay's answer didn't decrypt."))
        }
        val out = Headers.Builder()
        for (pair in answer.headers) if (pair.size == 2) out.addUnsafeNonAscii(pair[0], pair[1])
        val outHeaders = out.build()
        return Response.Builder()
            .request(request)
            .protocol(Protocol.HTTP_1_1)
            .code(answer.status)
            .message("")
            .headers(outHeaders)
            .body((Base64.getDecoder().decode(answer.body)).toResponseBody(outHeaders["content-type"]?.toMediaTypeOrNull()))
            .build()
    }

    companion object {
        private val JSON = "application/json".toMediaType()

        /** The relay answers within 45 s, with the hub's reply or its own timeout. */
        fun client(base: OkHttpClient = OkHttpClient()): OkHttpClient =
            base.newBuilder().callTimeout(50, TimeUnit.SECONDS).readTimeout(50, TimeUnit.SECONDS).build()
    }
}
