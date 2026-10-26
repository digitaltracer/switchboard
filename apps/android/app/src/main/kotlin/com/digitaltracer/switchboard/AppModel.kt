package com.digitaltracer.switchboard

import android.content.Context
import android.os.Build
import android.provider.Settings
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.digitaltracer.switchboard.api.models.AgentView
import com.digitaltracer.switchboard.api.models.EndedAgent
import com.digitaltracer.switchboard.api.models.Meta
import com.digitaltracer.switchboard.api.models.Project
import com.digitaltracer.switchboard.api.models.Session
import com.digitaltracer.switchboard.api.models.TargetView
import com.digitaltracer.switchboard.core.HerdrSession
import com.digitaltracer.switchboard.core.DeviceKeys
import com.digitaltracer.switchboard.core.Hub
import com.digitaltracer.switchboard.core.HubError
import com.digitaltracer.switchboard.core.KeyRole
import com.digitaltracer.switchboard.core.Pairing
import com.digitaltracer.switchboard.core.PairingLink
import com.digitaltracer.switchboard.core.displayName
import java.net.URI
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.MapSerializer
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.json.Json

/** Where the home screen has navigated to; the stack is kept here so it survives rotation. */
sealed interface Route {
    data class SessionPage(val id: String) : Route
    data class AgentPage(val machine: String, val name: String) : Route
    data class HerdrSessionPage(val key: HerdrSession.Key) : Route
    data object AllSessions : Route
    data class MachinePage(val machine: String) : Route
    data object Machines : Route
    data object Projects : Route
    data object Settings : Route
    data class Conversation(val source: ConversationSource, val title: String) : Route
}

/** The app's state: the pairing, the live session list and what the user is doing. */
class AppModel(context: Context, val keys: DeviceKeys) {
    sealed interface Phase {
        data object Unpaired : Phase
        /** Waiting for the admin to confirm the fingerprint in the terminal. */
        data class Confirming(val fingerprint: String) : Phase
        data object Paired : Phase
    }

    @Serializable
    data class Said(val text: String, val at: Long)

    private val prefs = context.getSharedPreferences("switchboard", Context.MODE_PRIVATE)
    private val scope = MainScope()
    private var pairJob: Job? = null
    private var pollJob: Job? = null

    /** The hub compares this with its oldest supported build. */
    val build: Int = context.packageManager.getPackageInfo(context.packageName, 0).longVersionCode.toInt()

    /** What the hub lists this phone as, until the user changes it. */
    val defaultName: String = Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME) ?: Build.MODEL

    var phase by mutableStateOf<Phase>(Phase.Unpaired)
        private set
    var pairing by mutableStateOf<Pairing?>(null)
        private set
    var hub by mutableStateOf<Hub?>(null)
        private set
    var pairingError by mutableStateOf<String?>(null)
    var sessions by mutableStateOf<List<Session>>(emptyList())
        private set
    var connected by mutableStateOf(false)
        private set
    var meta by mutableStateOf<Meta?>(null)
        private set

    /** Herdr agents Switchboard didn't start, refreshed while a list is on screen. */
    var agents by mutableStateOf<List<AgentView>>(emptyList())
        private set
    /** Those that ended in the last day. */
    var ended by mutableStateOf<List<EndedAgent>>(emptyList())
        private set
    var projects by mutableStateOf<List<Project>>(emptyList())
        private set

    /**
     * Bumped each time the app comes back to the foreground; screens restart their polls on it,
     * since a long-poll begun before the phone slept may be on a dead connection.
     */
    var wakes by mutableStateOf(0)
        private set

    /** What this phone last said to each session or agent. Kept on the phone only: the hub doesn't store prompts. */
    var said by mutableStateOf<Map<String, Said>>(emptyMap())
        private set

    /** Where text that names no project goes. */
    var current by mutableStateOf<TargetView?>(null)

    data class Sent(val target: TargetView?, val text: String, val started: Boolean, val at: Long)

    /** The last message sent from the home screen's voice or keyboard, shown there for a while. */
    var lastSent by mutableStateOf<Sent?>(null)

    enum class Appearance(val label: String) { System("System"), Light("Light"), Dark("Dark") }

    /** Light, dark or the phone's own setting; kept when the hub is forgotten. */
    var appearance by mutableStateOf(
        prefs.getString(APPEARANCE, null)?.let { v -> Appearance.entries.firstOrNull { it.name == v } } ?: Appearance.System,
    )
        private set

    fun chooseAppearance(a: Appearance) {
        appearance = a
        prefs.edit().putString(APPEARANCE, a.name).apply()
    }

    /** A pairing link opened from outside the app, for the pairing screen to pick up. */
    var openedLink by mutableStateOf<String?>(null)

    val path = mutableStateListOf<Route>()

    fun pop() {
        if (path.isNotEmpty()) path.removeAt(path.lastIndex)
    }

    /** What the back link says: the title of the screen underneath, as on iOS. */
    val backLabel: String
        get() = when (val under = path.getOrNull(path.size - 2)) {
            null -> "Agents"
            is Route.SessionPage -> sessions.firstOrNull { it.id == under.id }?.label ?: "Back"
            is Route.AgentPage -> under.name
            is Route.HerdrSessionPage -> under.key.name
            Route.AllSessions -> "All sessions"
            is Route.MachinePage -> under.machine
            Route.Machines -> "Machines"
            Route.Projects -> "Projects"
            Route.Settings -> "Settings"
            is Route.Conversation -> under.title
        }

    init {
        prefs.getString(SAID, null)?.let { said = runCatching { Json.decodeFromString(saidSerializer, it) }.getOrDefault(emptyMap()) }
        prefs.getString(PAIRING, null)
            ?.let { runCatching { Json.decodeFromString(Pairing.serializer(), it) }.getOrNull() }
            ?.let(::use)
    }

    private fun use(p: Pairing) {
        pairing = p
        hub = Hub.paired(p, keys, build)
        phase = Phase.Paired
        startPolling()
    }

    // Pairing.

    /** Checks the link's hub, claims the code with fresh keys, then waits for the admin's yes. */
    fun pair(link: PairingLink, name: String) {
        pairingError = null
        pairJob = scope.launch {
            val open = Hub.forLink(link, keys, build)
            try {
                val meta = open.meta()
                if (meta.hubId != link.hubId) {
                    pairingError = "This code is for a different hub than the one at ${host(link.hubUrl)}."
                    return@launch
                }
                if (meta.minClientBuild > build) {
                    pairingError = "This hub needs a newer version of the app."
                    return@launch
                }
                withContext(Dispatchers.Default) {
                    keys.reset()
                    KeyRole.entries.forEach(keys::publicKey)
                }
                val claimed = open.claim(link.code, name, build)
                // The hub's fingerprint must be the one our own keys give, or someone swapped them.
                if (open.localFingerprint(claimed.hubId) != claimed.fingerprint) {
                    pairingError = "The hub reported keys that aren't this phone's. Don't pair; tell the admin."
                    return@launch
                }
                phase = Phase.Confirming(claimed.fingerprint)
                var state = claimed.state
                // The code lives ten minutes; Cancel ends the wait sooner.
                val deadline = System.currentTimeMillis() + 600_000
                while (state == "pending" && System.currentTimeMillis() < deadline) {
                    state = open.pairState(claimed.deviceId)
                }
                if (state == "pending") state = "not confirmed in time"
                if (state != "active") {
                    phase = Phase.Unpaired
                    pairingError = "Pairing was $state at the hub. Ask for a new code."
                    return@launch
                }
                val p = Pairing(link.hubUrl, claimed.hubId, claimed.deviceId, name, link.relay)
                prefs.edit().putString(PAIRING, Json.encodeToString(Pairing.serializer(), p)).apply()
                use(p)
            } catch (e: CancellationException) {
                phase = Phase.Unpaired
                throw e
            } catch (e: Exception) {
                phase = Phase.Unpaired
                pairingError = e.message ?: e.javaClass.simpleName
            }
        }
    }

    fun cancelPairing() {
        pairJob?.cancel()
    }

    /** Forgets the hub and this phone's keys. The hub keeps the device until it's revoked. */
    fun forget() {
        pollJob?.cancel()
        prefs.edit().remove(PAIRING).remove(SAID).apply()
        runCatching { keys.reset() }
        said = emptyMap()
        pairing = null
        hub = null
        sessions = emptyList()
        agents = emptyList()
        ended = emptyList()
        projects = emptyList()
        meta = null
        connected = false
        current = null
        lastSent = null
        path.clear()
        phase = Phase.Unpaired
    }

    /** The app is in the foreground again: restart every poll. */
    fun becameActive() {
        if (phase != Phase.Paired) return
        wakes++
        startPolling()
    }

    /** Nothing polls while the app isn't on screen; push covers that once it's built. */
    fun wentToBackground() {
        pollJob?.cancel()
    }

    /** Remembers a message this phone sent, for the session page and the home screen. */
    fun record(sent: String, to: TargetView?, started: Boolean) {
        lastSent = Sent(to, sent, started, System.currentTimeMillis())
        if (to == null) return
        val next = said + (saidKey(to) to Said(sent, System.currentTimeMillis()))
        said = if (next.size > MAX_SAID) next - next.minBy { it.value.at }.key else next
        prefs.edit().putString(SAID, Json.encodeToString(saidSerializer, said)).apply()
    }

    /** Names the speech recognizer should expect: projects, sessions and agents, as written and as spoken. */
    val vocabulary: List<String>
        get() {
            val names = mutableListOf("Switchboard", "Herdr", "Claude", "Codex", "worktree")
            projects.forEach { names += listOf(it.id) + it.aliases }
            sessions.forEach { names += listOfNotNull(it.label, it.branch) }
            agents.filter { it.displayName == it.name }.forEach { names += it.name.substringAfterLast('/') }
            val seen = mutableSetOf<String>()
            return names.flatMap { listOf(it, it.replace(Regex("[-_/]+"), " ")) }.filter { seen.add(it.lowercase()) }.take(100)
        }

    suspend fun refreshAgents() {
        val hub = hub ?: return
        runCatching { hub.agents() }.onSuccess {
            agents = it.agents
            ended = it.ended.orEmpty()
        }
    }

    /** Every Herdr session with its agents, whoever started them. Ended agents count from [since], or all that are known when null. */
    fun herdrSessions(machine: String? = null, since: Long?): List<HerdrSession> =
        HerdrSession.group(sessions, agents, ended, machine, since)

    // The session list, long-polled: the hub answers when something newer than our revision exists.

    fun startPolling() {
        pollJob?.cancel()
        pollJob = scope.launch {
            var revision: Long? = null
            while (isActive) {
                val hub = hub ?: return@launch
                try {
                    if (meta == null) meta = hub.meta()
                    if (projects.isEmpty()) runCatching { hub.projects() }.onSuccess { projects = it }
                    val list = hub.sessions(after = revision)
                    sessions = list.sessions
                    revision = list.revision
                    connected = true
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    connected = false
                    if (handleRevoked(e)) return@launch
                    delay(3_000)
                }
            }
        }
    }

    /** A refused token refresh means the hub no longer knows this device. */
    fun handleRevoked(e: Exception): Boolean {
        if ((e as? HubError)?.code != "unknown_device") return false
        forget()
        pairingError = "This phone was revoked at the hub. Pair it again."
        return true
    }

    private fun host(url: String) = runCatching { URI(url).host }.getOrNull() ?: "that address"

    companion object {
        private const val PAIRING = "pairing"
        private const val SAID = "said"
        private const val APPEARANCE = "appearance"
        private const val MAX_SAID = 50
        private val saidSerializer = MapSerializer(String.serializer(), Said.serializer())

        fun saidKey(t: TargetView) = t.sessionId ?: "agent:${t.machineId}/${t.agent ?: t.label}"
    }
}
