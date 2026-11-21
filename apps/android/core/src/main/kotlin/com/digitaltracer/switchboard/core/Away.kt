package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.AwayAgentView
import com.digitaltracer.switchboard.api.models.AwayFactView

/** "While you were away": since this phone last had the app open. */
object AwayDigest {
    /** Shorter gaps aren't "away". */
    const val MIN_AWAY_MS = 10 * 60 * 1000L

    /** "1 h 40 m", "12 min", "under a minute". */
    fun duration(ms: Long): String {
        val m = (ms / 60_000).toInt()
        return when {
            m < 1 -> "under a minute"
            m < 60 -> "$m min"
            m % 60 == 0 -> "${m / 60} h"
            else -> "${m / 60} h ${m % 60} m"
        }
    }

    /** One line for the Inbox card: what it's doing now, or what it last did. */
    fun line(a: AwayAgentView, now: Long): String {
        val reply = a.lastReply?.let { "\"$it\"" }
        return when (a.state) {
            "waiting" -> {
                val at = a.facts.lastOrNull { it.kind == "waiting" }?.at ?: now
                "Waiting ${duration(now - at)}" + (reply?.let { ": $it" } ?: "")
            }
            "error" -> "Stopped with an error"
            "working" -> {
                val allowed = a.facts.count { it.kind == "allowed" }
                "Still working" + if (allowed > 0) " · you allowed $allowed ${if (allowed == 1) "command" else "commands"} meanwhile" else ""
            }
            else -> if (a.facts.any { it.kind == "finished" }) "Finished" + (reply?.let { ": $it" } ?: "") else a.facts.lastOrNull()?.text ?: "Nothing new"
        }
    }

    /** Its dot: needs you, stopped, working or done. */
    fun tone(a: AwayAgentView) = when (a.state) {
        "waiting" -> "need"
        "error" -> "bad"
        "working" -> "working"
        else -> "done"
    }

    /** Rows the agent page shows at most. */
    const val MAX_ROWS = 5

    /**
     * The facts as a short story: consecutive turns collapse into one "Finished" row at the last
     * finish, the "Started working" between them goes, and only the newest rows are kept.
     */
    fun timeline(facts: List<AwayFactView>): List<AwayFactView> {
        val out = mutableListOf<AwayFactView>()
        var turns = 0
        var pending: AwayFactView? = null
        fun flush() {
            val p = pending ?: return
            out += AwayFactView(p.at, "finished", if (turns == 1) "Finished its turn" else "Finished $turns turns")
            pending = null
            turns = 0
        }
        for (f in facts) {
            when {
                f.kind == "working" && (pending != null || out.lastOrNull()?.kind == "working") -> continue
                f.kind == "finished" -> {
                    if (out.lastOrNull()?.kind == "working" && pending == null) out.removeAt(out.lastIndex)
                    turns++
                    pending = f
                }
                else -> {
                    flush()
                    out += f
                }
            }
        }
        flush()
        return out.takeLast(MAX_ROWS)
    }
}
