package com.digitaltracer.switchboard.core

/** Whether the phone offers to rename something or start an agent somewhere. */
sealed interface Offer {
    data object Yes : Offer

    /** It can't be done from the phone; the page says why. */
    data class No(val why: String) : Offer

    /** Nothing to offer or say: it has ended, or the hub or the machine's runner is too old to tell. */
    data object Hidden : Offer
}

/** The id of the space it runs in on its machine, when the hub and runner say. */
val HerdrSession.Agent.spaceId: String?
    get() = when (this) {
        is HerdrSession.Agent.Task -> session.spaceId
        is HerdrSession.Agent.Live -> agent.spaceId
        is HerdrSession.Agent.Ended -> null
    }

/**
 * A session is renamed in the hub. An agent Switchboard didn't start is renamed where it runs,
 * when its runner says it can be.
 */
val HerdrSession.Agent.rename: Offer
    get() = when (this) {
        is HerdrSession.Agent.Task -> if (session.canRename == true && session.lifecycle != "deleted") Offer.Yes else Offer.Hidden
        is HerdrSession.Agent.Live -> when (agent.canRename) {
            true -> Offer.Yes
            false -> Offer.No(Names.AT_THE_MAC)
            null -> Offer.Hidden
        }
        is HerdrSession.Agent.Ended -> Offer.Hidden
    }

/** One in a Herdr terminal that Switchboard didn't start: Herdr restricts its names. */
val HerdrSession.Agent.isHerdrAgent: Boolean
    get() = this is HerdrSession.Agent.Live && agent.herdrSession != HerdrSession.HERDR_APP

private const val SEVERAL_SHARE = "Several spaces share this name."

/** The ids its live agents give for their space. */
private val AgentSpace.spaceIds: Set<String?> get() = live.map { it.spaceId }.toSet()

/** The one id its live agents' space goes by; null when none says, or they disagree. */
val AgentSpace.spaceId: String? get() = spaceIds.singleOrNull()

/**
 * A Herdr app space, or a Herdr workspace the phone shows by name. A space shown under its Herdr
 * session's name can't be renamed: Herdr has no way to rename a session.
 */
val AgentSpace.rename: Offer
    get() = when {
        live.isEmpty() -> Offer.Hidden
        !isHerdrApp && key.space == null -> Offer.No("This Herdr session's name is fixed. Rename a workspace at the Mac.")
        key.space == null -> Offer.Hidden
        else -> shared("Rename one at the Mac.")
    }

/**
 * A new session's tab can open in a Herdr app space. In a Herdr terminal each session gets a
 * Herdr session of its own, so nothing is added to one from the phone.
 */
val AgentSpace.newAgent: Offer
    get() = when {
        live.isEmpty() -> Offer.Hidden
        !isHerdrApp -> Offer.No("New agents here start at the Mac.")
        key.space == null -> Offer.Hidden
        else -> shared("Start it at the Mac.")
    }

private fun AgentSpace.shared(advice: String): Offer {
    val ids = spaceIds
    return when {
        ids == setOf<String?>(null) -> Offer.Hidden
        ids.size == 1 -> Offer.Yes
        else -> Offer.No("$SEVERAL_SHARE $advice")
    }
}

/** The project its Switchboard sessions share, to start another in; null when they differ. */
val AgentSpace.project: String?
    get() = live.mapNotNull { (it as? HerdrSession.Agent.Task)?.session?.projectId }.toSet().singleOrNull()

/** Names the owner gives from the phone. */
object Names {
    const val MAX_LENGTH = 40
    const val AT_THE_MAC = "Rename it at the Mac."
    const val HERDR_AGENT_RULE = "Start with a lowercase letter, then lowercase letters, digits, - or _. 32 at most."

    /** What is wrong with a new name, or null. [herdrAgent] is an agent in a Herdr terminal. */
    fun problem(name: String, herdrAgent: Boolean = false): String? {
        val n = name.trim()
        if (n.isEmpty()) return "Give it a name."
        if (herdrAgent) {
            val ok = n.first() in 'a'..'z' && n.length <= 32 &&
                n.drop(1).all { it in 'a'..'z' || it in '0'..'9' || it == '-' || it == '_' }
            return if (ok) null else HERDR_AGENT_RULE
        }
        if (n.codePointCount(0, n.length) > MAX_LENGTH) return "Keep it to $MAX_LENGTH characters."
        if (n.any { it == '/' || it.isISOControl() }) return "A name can't contain a slash or a line break."
        return null
    }

    /** A rename that failed, in words for the sheet. */
    fun failure(e: Throwable): String = when ((e as? HubError)?.code) {
        "name_taken" -> "That name is taken."
        "machine_offline" -> "That machine is offline."
        "not_found", "unknown_session" -> "It's no longer there."
        "busy" -> "The layout changed. Try again."
        "runner_too_old" -> "Update Switchboard on that machine first."
        "outcome_unknown" -> "No answer. Check the name in a moment."
        else -> e.message ?: e.javaClass.simpleName
    }
}
