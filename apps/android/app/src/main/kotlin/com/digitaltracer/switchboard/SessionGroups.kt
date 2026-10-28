package com.digitaltracer.switchboard

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.core.HerdrSession
import com.digitaltracer.switchboard.core.agentStatus
import com.digitaltracer.switchboard.core.needsYou
import com.digitaltracer.switchboard.core.shortName
import kotlinx.coroutines.delay

/** "3 agents · 1 working", or when it ended. */
val HerdrSession.summary: String
    get() {
        if (isEnded) return ended(endedAt)
        val parts = mutableListOf("${live.size} ${if (live.size == 1) "agent" else "agents"}")
        val needs = live.count { it.needsYou }
        val working = live.count { it.agentStatus == "working" }
        if (needs > 0) parts += "$needs need${if (needs == 1) "s" else ""} you"
        if (working > 0) parts += "$working working"
        if (needs == 0 && working == 0) parts += if (live.size == 1) "idle" else "all idle"
        return parts.joinToString(" · ")
    }

val HerdrSession.status: SessionStatus
    get() = when {
        isEnded -> SessionStatus.ended
        live.any { it.needsYou } -> SessionStatus.agent("blocked")
        live.any { it.agentStatus == "working" } -> SessionStatus.agent("working")
        else -> SessionStatus.agent("idle")
    }

val HerdrSession.Agent.status: SessionStatus
    get() = when (this) {
        is HerdrSession.Agent.Task -> SessionStatus.of(session)
        is HerdrSession.Agent.Live -> SessionStatus.agent(agent.agentStatus)
        is HerdrSession.Agent.Ended -> SessionStatus.ended
    }

val HerdrSession.Agent.isEnded get() = agentStatus == "ended"

/** "Working · Session approval", "Needs you · Claude · fix-flaky", or "Ended 2h ago · …". */
val HerdrSession.Agent.subtitle: String
    get() = when (this) {
        is HerdrSession.Agent.Task ->
            listOfNotNull(if (isEnded) ended(session.updatedAt) else status.text, session.agent.capitalized(), session.branch).joinToString(" · ")
        is HerdrSession.Agent.Live -> listOfNotNull(status.text, agent.title ?: agent.kind?.capitalized()).joinToString(" · ")
        is HerdrSession.Agent.Ended -> listOfNotNull(ended(agent.endedAt), agent.title ?: agent.kind?.capitalized()).joinToString(" · ")
    }

/** Where tapping it goes; an agent that ended outside Switchboard has nothing more to show. */
val HerdrSession.Agent.route: Route?
    get() = when (this) {
        is HerdrSession.Agent.Task -> Route.SessionPage(session.id)
        is HerdrSession.Agent.Live -> Route.AgentPage(agent.machineId, agent.name)
        is HerdrSession.Agent.Ended -> null
    }

private fun ended(ms: Long) = Ago.short(ms).let { if (it == "now") "Ended just now" else "Ended $it ago" }

/** One Herdr session, as a row of a sessions list. */
@Composable
fun SessionRow(model: AppModel, session: HerdrSession, showMachine: Boolean, first: Boolean) {
    val status = session.status
    ListRow(
        status.symbol, if (session.isEnded) palette.secondary else status.color, session.key.name,
        if (showMachine) "${session.summary} · ${session.key.machine}" else session.summary, "", first,
        tag = "session-${session.key.name}", background = if (session.isEnded) palette.endedRow else Color.Transparent,
    ) { model.path.add(Route.HerdrSessionPage(session.key)) }
}

/** A Herdr session's agents: running ones first, then those that ended today. */
@Composable
fun HerdrSessionScreen(model: AppModel, key: HerdrSession.Key) {
    LaunchedEffect(key, model.wakes) {
        while (true) {
            model.refreshAgents()
            delay(6_000)
        }
    }
    val session = model.herdrSessions(key.machine, HerdrSession.startOfToday()).firstOrNull { it.key == key } ?: HerdrSession(key)
    GroupedList(model, key.name, onRefresh = model::refreshAgents) {
        item {
            InsetGroup("Agents") {
                if (session.agents.isEmpty()) {
                    GroupRow(true) { Text("No agents in this session.", style = Type.body, color = palette.secondary) }
                }
                session.agents.forEachIndexed { i, a ->
                    val status = a.status
                    GroupRow(i == 0, a.route?.let { r -> { model.path.add(r) } }, if (a.isEnded) palette.endedRow else Color.Transparent) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Box(Modifier.width(26.dp), Alignment.Center) { Icon(status.symbol, null, Modifier.size(22.dp), tint = status.color) }
                            Spacer(Modifier.width(14.dp))
                            Column(Modifier.fillMaxWidth().testTag("agent-${a.shortName}"), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text(a.shortName, style = Type.headline, color = if (a.isEnded) palette.secondary else palette.label, maxLines = 1)
                                Text(a.subtitle, style = Type.subheadline, color = if (a.isEnded) palette.secondary else status.color, maxLines = 1)
                            }
                        }
                    }
                }
            }
        }
        item {
            Text(
                "On ${key.machine}. Open it there with: herdr session attach ${key.name}",
                Modifier.fillMaxWidth(), style = Type.footnote, color = palette.secondary,
            )
        }
    }
}
