package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.AgentView
import com.digitaltracer.switchboard.api.models.EndedAgent
import com.digitaltracer.switchboard.api.models.Session
import java.time.LocalDate
import java.time.ZoneId

/** A Herdr session and every agent in it, whoever started them: running ones first, then those that ended. */
data class HerdrSession(val key: Key, val live: List<Agent> = emptyList(), val ended: List<Agent> = emptyList()) {
    data class Key(val machine: String, val name: String)

    sealed interface Agent {
        /** One Switchboard started, with its worktree and approvals. */
        data class Task(val session: Session) : Agent
        data class Live(val agent: AgentView) : Agent
        data class Ended(val agent: EndedAgent) : Agent
    }

    /** No agent is left running in it. */
    val isEnded get() = live.isEmpty()
    val agents get() = live + ended

    /** When its last agent ended, for a session that has ended. */
    val endedAt get() = ended.maxOfOrNull { it.endedAt } ?: 0

    companion object {
        internal val liveLifecycles = setOf("starting", "running", "stopping")

        /**
         * Groups the hub's lists by machine and Herdr session. Ended agents count from [since], or
         * all of them when it is null; [machine] keeps only that machine's.
         */
        fun group(
            sessions: List<Session>, agents: List<AgentView>, ended: List<EndedAgent>,
            machine: String? = null, since: Long?,
        ): List<HerdrSession> {
            val live = LinkedHashMap<Key, MutableList<Agent>>()
            val gone = LinkedHashMap<Key, MutableList<Agent>>()
            fun add(a: Agent, m: String, session: String?, isLive: Boolean) {
                if (machine != null && machine != m) return
                val key = Key(m, session ?: "default")
                live.getOrPut(key) { mutableListOf() }
                gone.getOrPut(key) { mutableListOf() }
                (if (isLive) live else gone).getValue(key) += a
            }
            for (a in agents) add(Agent.Live(a), a.machineId, a.herdrSession, true)
            for (s in sessions.sortedBy { it.createdAt }) {
                val isLive = s.lifecycle in liveLifecycles
                if (!isLive && since != null && s.updatedAt < since) continue
                add(Agent.Task(s), s.machineId, s.herdrSession, isLive)
            }
            for (e in ended) {
                if (since != null && e.endedAt < since) continue
                add(Agent.Ended(e), e.machineId, e.herdrSession, false)
            }
            val all = live.keys.map { HerdrSession(it, live.getValue(it), gone.getValue(it).sortedByDescending { a -> a.endedAt }) }
            val running = all.filter { !it.isEnded }.sortedWith(compareBy({ it.key.name }, { it.key.machine }))
            val done = all.filter { it.isEnded }.sortedByDescending { it.endedAt }
            return running + done
        }

        /** Where today began on this phone, in the hub's milliseconds. */
        fun startOfToday(): Long = LocalDate.now().atStartOfDay(ZoneId.systemDefault()).toInstant().toEpochMilli()
    }
}

val HerdrSession.Agent.machineId: String
    get() = when (this) {
        is HerdrSession.Agent.Task -> session.machineId
        is HerdrSession.Agent.Live -> agent.machineId
        is HerdrSession.Agent.Ended -> agent.machineId
    }

/** Its name within its session: the Herdr name without the session, or what its terminal says when it has none. */
val HerdrSession.Agent.shortName: String
    get() = when (this) {
        is HerdrSession.Agent.Task -> session.label
        is HerdrSession.Agent.Live -> short(agent.name, agent.title)
        is HerdrSession.Agent.Ended -> short(agent.name, agent.title)
    }

/** Herdr's view of it: `idle`, `working`, `blocked`, `done`, or `ended`. */
val HerdrSession.Agent.agentStatus: String
    get() = when (this) {
        is HerdrSession.Agent.Task -> if (session.lifecycle in HerdrSession.liveLifecycles) session.agentStatus else "ended"
        is HerdrSession.Agent.Live -> agent.agentStatus
        is HerdrSession.Agent.Ended -> "ended"
    }

val HerdrSession.Agent.needsYou: Boolean
    get() = when (this) {
        is HerdrSession.Agent.Task -> session.pendingApprovals.isNotEmpty() || session.hookFailed || session.agentStatus == "blocked"
        is HerdrSession.Agent.Live -> agent.agentStatus == "blocked"
        is HerdrSession.Agent.Ended -> false
    }

val HerdrSession.Agent.endedAt: Long
    get() = when (this) {
        is HerdrSession.Agent.Task -> session.updatedAt
        is HerdrSession.Agent.Live -> 0
        is HerdrSession.Agent.Ended -> agent.endedAt
    }

private fun short(name: String, title: String?): String {
    val last = name.substringAfterLast('/')
    return if (last.contains(':') && !title.isNullOrEmpty()) title else last
}
