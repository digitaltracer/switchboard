package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.AgentView
import com.digitaltracer.switchboard.api.models.EndedAgent
import com.digitaltracer.switchboard.api.models.Session

/**
 * One group of agents as the phone lists it, wherever it lives: a space in the Herdr app, or a
 * workspace of a Herdr terminal session, `session › workspace`. A terminal session with one
 * workspace, or only numbered ones, is one space under the session's name; its numbered
 * workspaces otherwise go under the session's name alone.
 */
data class AgentSpace(
    val key: Key,
    /** `atlas-web`, `main › kraft`, `kraft-7c1e`. */
    val title: String,
    val live: List<HerdrSession.Agent>,
    val ended: List<HerdrSession.Agent>,
) {
    /** [space] is null for the whole of a terminal session, or the Herdr app's agents outside every space. */
    data class Key(val machine: String, val session: String, val space: String?)

    val isHerdrApp get() = key.session == HerdrSession.HERDR_APP

    /** `hc` for the Herdr app, `herdr` for a Herdr terminal. */
    val backend get() = if (isHerdrApp) "hc" else "herdr"
    val isEnded get() = live.isEmpty()
    val agents get() = live + ended
    val endedAt get() = ended.maxOfOrNull { it.endedAt } ?: 0

    /** Holds a session Switchboard started. */
    val fromSwitchboard get() = agents.any { it is HerdrSession.Agent.Task }

    /** The Herdr terminal session to attach to; none for the Herdr app. */
    val attach: String? get() = if (isHerdrApp) null else key.session

    /** The name to show for one of its agents elsewhere, e.g. `atlas-web › search`. */
    fun label(a: HerdrSession.Agent): String {
        val name = a.nameInSpace
        return if (name == title) title else "$title › $name"
    }

    companion object {
        /** Every space, live ones in title order, then those that ended, latest first. */
        fun group(
            sessions: List<Session>, agents: List<AgentView>, ended: List<EndedAgent>,
            machine: String? = null, since: Long?,
        ): List<AgentSpace> {
            val all = HerdrSession.group(sessions, agents, ended, machine, since).flatMap(::split)
            val live = all.filter { !it.isEnded }.sortedWith(compareBy({ it.title.lowercase() }, { it.key.machine }))
            val done = all.filter { it.isEnded }.sortedByDescending { it.endedAt }
            return live + done
        }

        internal fun split(g: HerdrSession): List<AgentSpace> {
            val m = g.key.machine
            val spaces = g.spaces
            if (g.isHerdrApp) {
                return spaces.map { s -> AgentSpace(Key(m, g.key.name, s.name), s.name ?: "Outside any space", s.live, s.ended) }
            }
            val named = spaces.filter { it.name != null }
            val unnamed = spaces.any { it.name == null }
            if (named.isEmpty() || (named.size == 1 && !unnamed)) {
                return listOf(AgentSpace(Key(m, g.key.name, null), g.title, g.live, g.ended))
            }
            return spaces.map { s ->
                AgentSpace(Key(m, g.key.name, s.name), s.name?.let { "${g.title} › $it" } ?: g.title, s.live, s.ended)
            }
        }
    }
}

/** The space holding the agent [match] picks out. */
fun List<AgentSpace>.holding(match: (HerdrSession.Agent) -> Boolean): AgentSpace? = firstOrNull { it.agents.any(match) }
