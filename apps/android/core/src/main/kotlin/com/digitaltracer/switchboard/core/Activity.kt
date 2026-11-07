package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.Agents

/**
 * Where home tiles get their last line: the hub's activity list for machines it covers, else
 * the agent's conversation, read at most every [FallbackPacer.INTERVAL_MS].
 */
class TileLines private constructor(
    private val machines: Set<String>,
    private val bySession: Map<String, String?>,
    private val byAgent: Map<String, String?>,
) {
    sealed interface Line {
        /** From the activity list; null when the agent has said nothing yet. */
        data class Known(val line: String?) : Line
        /** Its machine can't say, or the hub is older: read the conversation. */
        data object Unknown : Line
    }

    fun line(agent: HerdrSession.Agent): Line = when (agent) {
        is HerdrSession.Agent.Task ->
            if (agent.session.machineId in machines) Line.Known(bySession[agent.session.id]) else Line.Unknown
        is HerdrSession.Agent.Live ->
            if (agent.agent.machineId in machines) Line.Known(byAgent["${agent.agent.machineId}/${agent.agent.name}"]) else Line.Unknown
        is HerdrSession.Agent.Ended -> Line.Known(null)
    }

    companion object {
        /** Null when the hub sent no activity, as older hubs don't. */
        fun of(list: Agents): TileLines? {
            val activity = list.activity ?: return null
            return TileLines(
                machines = list.activityMachines.orEmpty().toSet(),
                bySession = activity.filter { it.sessionId != null }.associate { it.sessionId!! to it.line },
                byAgent = activity.associate { "${it.machineId}/${it.name}" to it.line },
            )
        }
    }
}

/** When each tile last read its conversation, so one activity doesn't cover reads it at most every [INTERVAL_MS]. */
class FallbackPacer {
    private val last = mutableMapOf<String, Long>()

    /** Whether tile [id] may read now; if so, the read counts from [nowMs]. */
    fun due(id: String, nowMs: Long = System.currentTimeMillis()): Boolean {
        val at = last[id]
        if (at != null && nowMs - at < INTERVAL_MS) return false
        last[id] = nowMs
        return true
    }

    companion object {
        const val INTERVAL_MS = 30_000L
    }
}
