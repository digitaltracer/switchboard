package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.apis.AgentsApi
import com.digitaltracer.switchboard.api.apis.ApprovalsApi
import com.digitaltracer.switchboard.api.apis.AuthApi
import com.digitaltracer.switchboard.api.apis.DevicesApi
import com.digitaltracer.switchboard.api.apis.MessagesApi
import com.digitaltracer.switchboard.api.apis.MetaApi
import com.digitaltracer.switchboard.api.apis.PairingApi
import com.digitaltracer.switchboard.api.apis.SessionsApi
import com.digitaltracer.switchboard.api.infrastructure.ClientError
import com.digitaltracer.switchboard.api.infrastructure.ClientException
import com.digitaltracer.switchboard.api.infrastructure.Serializer
import com.digitaltracer.switchboard.api.infrastructure.ServerError
import com.digitaltracer.switchboard.api.infrastructure.ServerException
import com.digitaltracer.switchboard.api.models.AgentView
import com.digitaltracer.switchboard.api.models.ApiError
import com.digitaltracer.switchboard.api.models.Changes
import com.digitaltracer.switchboard.api.models.DecisionKind
import com.digitaltracer.switchboard.api.models.DecisionRequest
import com.digitaltracer.switchboard.api.models.DecisionResponse
import com.digitaltracer.switchboard.api.models.DeleteResponse
import com.digitaltracer.switchboard.api.models.Device
import com.digitaltracer.switchboard.api.models.Machine
import com.digitaltracer.switchboard.api.models.MessagePreview
import com.digitaltracer.switchboard.api.models.MessagePreviewRequest
import com.digitaltracer.switchboard.api.models.MessageRequest
import com.digitaltracer.switchboard.api.models.MessageResponse
import com.digitaltracer.switchboard.api.models.Meta
import com.digitaltracer.switchboard.api.models.MuteRequest
import com.digitaltracer.switchboard.api.models.MuteResponse
import com.digitaltracer.switchboard.api.models.OperationStatus
import com.digitaltracer.switchboard.api.models.Output
import com.digitaltracer.switchboard.api.models.PairRequest
import com.digitaltracer.switchboard.api.models.PairResponse
import com.digitaltracer.switchboard.api.models.Project
import com.digitaltracer.switchboard.api.models.PromptRequest
import com.digitaltracer.switchboard.api.models.PromptResponse
import com.digitaltracer.switchboard.api.models.PushEnvironment
import com.digitaltracer.switchboard.api.models.PushTokenRequest
import com.digitaltracer.switchboard.api.models.Session
import com.digitaltracer.switchboard.api.models.Sessions
import com.digitaltracer.switchboard.api.models.StartRequest
import com.digitaltracer.switchboard.api.models.StartResponse
import com.digitaltracer.switchboard.api.models.StopRequest
import com.digitaltracer.switchboard.api.models.StopResponse
import com.digitaltracer.switchboard.api.models.TargetRef
import com.digitaltracer.switchboard.api.models.TargetView
import com.digitaltracer.switchboard.api.models.TokenChallengeRequest
import com.digitaltracer.switchboard.api.models.TokenRequest
import com.digitaltracer.switchboard.api.models.Transcript
import java.io.IOException
import java.net.URI
import java.net.URLDecoder
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationException
import okhttp3.Interceptor
import okhttp3.OkHttpClient

/** An error the hub answered with, by its stable code, or a failure to reach it. */
sealed class HubError(message: String) : Exception(message) {
    class Hub(override val code: String, message: String, val operationId: String?, val sessionId: String?) : HubError(message)

    /** No answer: the request may or may not have reached the hub. */
    class Unreachable(why: String) : HubError("Can't reach the hub: $why")

    object NotPaired : HubError("This phone isn't paired.")

    class Unexpected(why: String) : HubError(why)

    open val code: String? get() = null

    /** The request may have done its work; look it up by its id before calling it failed. */
    val outcomeUnknown: Boolean get() = this is Unreachable || code in setOf("outcome_unknown", "reconciling")
}

/** What a paired phone remembers about its hub. Nothing here is secret; the keys are. */
@Serializable
data class Pairing(
    /** The hub's own address, or the relay's when `relay` is set. */
    val hubUrl: String,
    val hubId: String,
    val deviceId: String,
    val name: String,
    /** Set when the phone reaches the hub through a relay. */
    val relay: RelayRoute? = null,
)

/**
 * The QR code `switchboard admin pair-device` shows: `switchboard://pair?url=…&code=…&hub=…` for
 * a hub with its own address, or `?relay=…&k=…&s=…&code=…&hub=…` for one behind a relay.
 */
data class PairingLink(val hubUrl: String, val code: String, val hubId: String, val relay: RelayRoute?) {
    companion object {
        fun parse(text: String): PairingLink? {
            val uri = runCatching { URI(text.trim()) }.getOrNull() ?: return null
            if (uri.scheme != "switchboard" || uri.host != "pair") return null
            val q = mutableMapOf<String, String>()
            for (part in uri.rawQuery.orEmpty().split('&')) {
                val name = part.substringBefore('=')
                if (part.contains('=') && name !in q) q[name] = decode(part.substringAfter('='))
            }
            val raw = q["relay"] ?: q["url"] ?: return null
            val url = runCatching { URI(raw) }.getOrNull() ?: return null
            if (url.scheme !in setOf("https", "http")) return null
            val code = q["code"]?.takeIf { it.isNotEmpty() } ?: return null
            val hub = q["hub"]?.takeIf { it.isNotEmpty() } ?: return null
            val base = raw.trimEnd('/')
            var relay: RelayRoute? = null
            if (q["relay"] != null) {
                relay = RelayRoute.of(base, q["k"] ?: return null, q["s"] ?: return null) ?: return null
            }
            return PairingLink(base, code, hub, relay)
        }

        // A literal `+` stays a `+`, as URLComponents reads it on iOS.
        private fun decode(s: String) = URLDecoder.decode(s.replace("+", "%2B"), Charsets.UTF_8)
    }
}

/** Short-lived access tokens, refreshed by signing a challenge with the auth key. */
internal class TokenSource(private val auth: AuthApi, private val deviceId: String, private val keys: DeviceKeys) {
    private val lock = Mutex()

    @Volatile
    private var current: Pair<String, Long>? = null

    val token: String? get() = current?.first

    suspend fun refresh(force: Boolean = false): String = lock.withLock {
        val soon = System.currentTimeMillis() + 60_000
        current?.let { (token, expiresAt) -> if (!force && expiresAt > soon) return token }
        val ch = Hub.unwrap { auth.tokenChallenge(TokenChallengeRequest(deviceId)) }
        val message = SignedMessage.token(ch.hubId, deviceId, ch.nonce, ch.expiresAt)
        val signature = keys.sign(message, KeyRole.Auth, "Sign in to Switchboard")
        val t = Hub.unwrap {
            auth.issueToken(TokenRequest(deviceId, ch.nonce, signature.base64Url, SignedMessage.VERSION))
        }
        current = t.accessToken to t.expiresAt
        t.accessToken
    }
}

/** The app's view of one hub. Every call re-checks with the hub; nothing is trusted from a push. */
class Hub private constructor(
    val pairing: Pairing?,
    url: String,
    relay: RelayRoute?,
    private val keys: DeviceKeys,
    build: Int,
    transport: OkHttpClient,
) {
    val baseUrl: String = url.trimEnd('/')
    private val tokens: TokenSource?
    private val meta: MetaApi
    private val pairingApi: PairingApi
    private val sessions: SessionsApi
    private val approvals: ApprovalsApi
    private val devicesApi: DevicesApi
    private val messages: MessagesApi
    private val agentsApi: AgentsApi

    init {
        fun client(auth: Interceptor?) = transport.newBuilder().apply {
            interceptors().add(0, Interceptor { it.proceed(it.request().newBuilder().header("x-switchboard-build", "$build").build()) })
            auth?.let { addInterceptor(it) }
            relay?.let { addInterceptor(RelayInterceptor(it, transport)) }
        }.build()
        val open = client(null)
        tokens = pairing?.let { TokenSource(AuthApi(baseUrl, open), it.deviceId, keys) }
        val authed = client(Interceptor { chain ->
            val token = tokens?.token
            chain.proceed(if (token == null) chain.request() else chain.request().newBuilder().header("Authorization", "Bearer $token").build())
        })
        meta = MetaApi(baseUrl, open)
        pairingApi = PairingApi(baseUrl, open)
        sessions = SessionsApi(baseUrl, authed)
        approvals = ApprovalsApi(baseUrl, authed)
        devicesApi = DevicesApi(baseUrl, authed)
        messages = MessagesApi(baseUrl, authed)
        agentsApi = AgentsApi(baseUrl, authed)
    }

    companion object {
        init {
            // Leave out absent optional fields, as the iOS client does, rather than sending null.
            Serializer.kotlinxSerializationJsonConfiguration = { explicitNulls = false }
        }

        /** A hub the phone hasn't paired with yet: only meta and pairing calls work. */
        fun unpaired(url: String, keys: DeviceKeys, build: Int, transport: OkHttpClient = directTransport()) =
            Hub(null, url, null, keys, build, transport)

        /** The hub a pairing link names, directly or through its relay. */
        fun forLink(link: PairingLink, keys: DeviceKeys, build: Int, transport: OkHttpClient = directTransport()) =
            Hub(null, link.hubUrl, link.relay, keys, build, transport(link.relay, transport))

        fun paired(pairing: Pairing, keys: DeviceKeys, build: Int, transport: OkHttpClient = directTransport()) =
            Hub(pairing, pairing.hubUrl, pairing.relay, keys, build, transport(pairing.relay, transport))

        private fun transport(relay: RelayRoute?, base: OkHttpClient) = if (relay != null) RelayInterceptor.client(base) else base

        /** Long-polls answer within 25 s, so a request silent for 40 s is on a dead connection. */
        fun directTransport(): OkHttpClient = OkHttpClient.Builder().readTimeout(40, TimeUnit.SECONDS).build()

        /** Runs a generated call, surfacing the hub's own error instead of the client's wrapper. */
        internal suspend fun <T> unwrap(call: suspend () -> T): T = try {
            call()
        } catch (e: ClientException) {
            throw hubError(e.statusCode, (e.response as? ClientError<*>)?.body as? String)
        } catch (e: ServerException) {
            throw hubError(e.statusCode, (e.response as? ServerError<*>)?.body as? String)
        } catch (e: RelayFailure) {
            throw e.error
        } catch (e: IOException) {
            // No HTTP response at all: the request may or may not have reached the hub.
            throw HubError.Unreachable(e.message ?: e.javaClass.simpleName)
        } catch (e: SerializationException) {
            throw HubError.Unexpected(e.message ?: "The hub's answer didn't decode.")
        }

        private fun hubError(status: Int, body: String?): HubError {
            val decoded = body?.let { runCatching { Serializer.kotlinxSerializationJson.decodeFromString<ApiError>(it) }.getOrNull() }
            return HubError.Hub(
                code = decoded?.error?.code ?: "http_$status",
                message = decoded?.error?.message ?: "The hub answered $status.",
                operationId = decoded?.error?.operationId,
                sessionId = decoded?.error?.sessionId,
            )
        }
    }

    /** Calls with a device token, and once more with a fresh one if the hub says it expired. */
    private suspend fun <T> authed(call: suspend () -> T): T {
        val tokens = tokens ?: throw HubError.NotPaired
        return unwrap {
            tokens.refresh()
            try {
                call()
            } catch (e: ClientException) {
                if (e.statusCode != 401) throw e
                tokens.refresh(force = true)
                call()
            }
        }
    }

    // Pairing.

    suspend fun meta(): Meta = unwrap { meta.getMeta() }

    /** Claims the code with this phone's public keys. Show the returned fingerprint. */
    suspend fun claim(code: String, name: String, build: Int): PairResponse {
        val auth = keys.publicKey(KeyRole.Auth).base64Url
        val approval = keys.publicKey(KeyRole.Approval).base64Url
        return unwrap { pairingApi.pair(PairRequest(code, name, auth, approval, build.toLong())) }
    }

    /** Waits up to 25 s for the admin to confirm; returns the device's state. */
    suspend fun pairState(deviceId: String): String = unwrap { pairingApi.pairStatus(deviceId, 25_000).state }

    /** The fingerprint this phone's keys give for `hubId`, to compare with the hub's. */
    fun localFingerprint(hubId: String): String = pairingFingerprint(
        hubId, keys.publicKey(KeyRole.Auth).base64Url, keys.publicKey(KeyRole.Approval).base64Url,
    )

    // Sessions.

    suspend fun projects(): List<Project> = authed { sessions.listProjects() }

    suspend fun machines(): List<Machine> = authed { sessions.listMachines() }

    /** With `after`, waits up to `waitMs` for a list newer than that revision. */
    suspend fun sessions(after: Long? = null, waitMs: Long = 25_000): Sessions =
        authed { sessions.listSessions(after, if (after == null) null else waitMs) }

    suspend fun session(id: String, after: Long? = null, waitMs: Long = 25_000): Session =
        authed { sessions.getSession(id, after, if (after == null) null else waitMs) }

    /**
     * The agent's terminal: the last [lines], or with [visible] only the screen as it is now. Only a
     * visible read leaves the agent's terminal still on its machine.
     */
    suspend fun output(id: String, lines: Int = 80, visible: Boolean = false): Output =
        authed { sessions.getOutput(id, lines, visible) }

    /**
     * A page of the session's conversation from its agent's own log: the latest, the part after an
     * earlier page's `end`, or the part before its `start`.
     */
    suspend fun transcript(id: String, after: Long? = null, before: Long? = null): Transcript =
        authed { sessions.getTranscript(id, after, before) }

    /** As [transcript], for an agent Switchboard didn't start. */
    suspend fun agentTranscript(machine: String, agent: String, after: Long? = null, before: Long? = null): Transcript =
        authed { agentsApi.getAgentTranscript(machine, agent, after, before) }

    /** Files the session changed since it started, committed or not. */
    suspend fun changes(id: String): Changes = authed { sessions.getChanges(id) }

    suspend fun start(
        project: String, prompt: String, machine: String?, agent: String?, notify: Boolean, requestId: String,
    ): StartResponse = authed { sessions.startSession(StartRequest(project, prompt, requestId, machine, agent, notify)) }

    suspend fun operation(requestId: String): OperationStatus = authed { sessions.getOperation(requestId) }

    suspend fun prompt(id: String, text: String, requestId: String): PromptResponse =
        authed { sessions.promptSession(id, PromptRequest(text, requestId)) }

    suspend fun stop(id: String): StopResponse = authed { sessions.stopSession(id, StopRequest(removeWorktreeIfClean = false)) }

    /** Stops the session if it's running, removes its worktree only if clean and merged, and deletes its Herdr session. */
    suspend fun delete(id: String): DeleteResponse = authed { sessions.deleteSession(id) }

    suspend fun mute(id: String, muted: Boolean): MuteResponse = authed { sessions.muteSession(id, MuteRequest(muted)) }

    // One message box: the hub decides between a new session and an existing one.

    /**
     * Routes `text`. `current` is where text that names no project goes; `projectId` or
     * `target` answers the hub's question from a previous call with the same `requestId`.
     */
    suspend fun message(
        text: String, requestId: String, current: TargetView?, projectId: String? = null, target: TargetView? = null,
    ): MessageResponse = authed {
        messages.postMessage(MessageRequest(text, requestId, current?.ref, projectId, target?.ref))
    }

    /** Where `message` would send `text`, without starting or sending anything. */
    suspend fun preview(text: String, current: TargetView?): MessagePreview =
        authed { messages.previewMessage(MessagePreviewRequest(text, current?.ref)) }

    /** Named Herdr agents Switchboard didn't start. */
    suspend fun agents(): List<AgentView> = authed { agentsApi.listAgents().agents }

    suspend fun agentOutput(machine: String, agent: String, lines: Int = 80, visible: Boolean = false): Output =
        authed { agentsApi.getAgentOutput(machine, agent, lines, visible) }

    // Approvals.

    suspend fun deny(approvalId: String, requestId: String): DecisionResponse =
        authed { approvals.decideApproval(approvalId, DecisionRequest(DecisionKind.deny, requestId = requestId)) }

    /** Signs a fresh challenge with the approval key, which asks for a fingerprint or the screen lock. */
    suspend fun approve(approvalId: String, reason: String, requestId: String): DecisionResponse {
        val pairing = pairing ?: throw HubError.NotPaired
        val ch = authed { approvals.approvalChallenge(approvalId) }
        val message = SignedMessage.approval(ch.hubId, pairing.deviceId, approvalId, ch.nonce, ch.expiresAt)
        val signature = keys.sign(message, KeyRole.Approval, reason)
        return authed {
            approvals.decideApproval(approvalId, DecisionRequest(
                DecisionKind.allow, ch.nonce, signature.base64Url, SignedMessage.VERSION, requestId,
            ))
        }
    }

    // Devices.

    suspend fun devices(): List<Device> = authed { devicesApi.listDevices() }

    suspend fun revoke(deviceId: String) = authed { devicesApi.revokeDevice(deviceId) }

    suspend fun setPushToken(token: String?, environment: PushEnvironment) =
        authed { devicesApi.setPushToken(PushTokenRequest(environment, token)) }
}

/** Its Herdr name, or what its terminal says when it has no name and goes by its pane. */
val AgentView.displayName: String
    get() {
        val title = title
        if (title.isNullOrEmpty() || !name.split('/').last().contains(':')) return name
        return title
    }

fun sessionTarget(s: Session) = TargetView("session", s.machineId, s.label, s.id, s.agentName)

fun agentTarget(machine: String, name: String, label: String? = null) = TargetView("agent", machine, label ?: name, agent = name)

/** What a request names this by. */
val TargetView.ref: TargetRef
    get() = TargetRef(sessionId, if (kind == "agent") machineId else null, if (kind == "agent") agent else null)
