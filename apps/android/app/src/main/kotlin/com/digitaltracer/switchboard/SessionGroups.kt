package com.digitaltracer.switchboard

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.core.AgentSpace
import com.digitaltracer.switchboard.core.HerdrSession
import com.digitaltracer.switchboard.core.agentStatus
import com.digitaltracer.switchboard.core.isNamed
import com.digitaltracer.switchboard.core.nameInSpace
import com.digitaltracer.switchboard.core.needsYou
import com.digitaltracer.switchboard.core.newAgent
import com.digitaltracer.switchboard.core.Offer
import com.digitaltracer.switchboard.core.project
import com.digitaltracer.switchboard.core.rename
import com.digitaltracer.switchboard.core.shownTitle
import com.digitaltracer.switchboard.core.spaceId
import java.util.UUID
import kotlinx.coroutines.delay

/** "3 agents · 1 needs you · 1 working", or when it ended. */
val AgentSpace.summary: String
    get() {
        if (isEnded) return "Ended ${Ago.long(endedAt)}"
        val parts = mutableListOf("${live.size} ${if (live.size == 1) "agent" else "agents"}")
        val needs = live.count { it.needsYou }
        val working = live.count { it.agentStatus == "working" }
        val done = live.count { it.agentStatus == "done" }
        if (live.size == 1) {
            parts += when {
                needs > 0 -> "needs you"
                working > 0 -> "working"
                done > 0 -> "done"
                else -> "idle"
            }
        } else {
            if (needs > 0) parts += "$needs need${if (needs == 1) "s" else ""} you"
            if (working > 0) parts += "$working working"
            if (needs == 0 && working == 0) parts += if (done > 0) "$done done" else "all idle"
        }
        if (fromSwitchboard) parts += "from Switchboard"
        return parts.joinToString(" · ")
    }

val AgentSpace.glyph: Glyph
    get() = when {
        isEnded -> Glyph.End
        live.any { it.needsYou } -> Glyph.Need
        live.any { it.agentStatus == "working" } -> Glyph.Work
        live.any { it.agentStatus == "done" } -> Glyph.Done
        else -> Glyph.Idle
    }

val HerdrSession.Agent.status: SessionStatus
    get() = when (this) {
        is HerdrSession.Agent.Task -> SessionStatus.of(session)
        is HerdrSession.Agent.Live -> SessionStatus.agent(agent.agentStatus)
        is HerdrSession.Agent.Ended -> SessionStatus.ended
    }

val HerdrSession.Agent.isEnded get() = agentStatus == "ended"

/** "Working · Add typeahead", "Needs you · Claude Code · sb/7c1e", or "Ended 2h ago · …". */
val HerdrSession.Agent.subtitle: String
    get() = when (this) {
        is HerdrSession.Agent.Task ->
            listOfNotNull(if (isEnded) "Ended ${Ago.long(session.updatedAt)}" else status.text, agentName(session.agent), session.branch).joinToString(" · ")
        is HerdrSession.Agent.Live -> {
            val kind = agentName(agent.kind)
            listOfNotNull(status.text, if (agent.isNamed) agent.shownTitle ?: kind else agent.shownTitle?.let { kind }).joinToString(" · ")
        }
        is HerdrSession.Agent.Ended -> listOfNotNull("Ended ${Ago.long(agent.endedAt)}", agent.title ?: agentName(agent.kind)).joinToString(" · ")
    }

/** Where tapping it goes; an agent that ended outside Switchboard has nothing more to show. */
val HerdrSession.Agent.route: Route?
    get() = when (this) {
        is HerdrSession.Agent.Task -> Route.SessionPage(session.id)
        is HerdrSession.Agent.Live -> Route.AgentPage(agent.machineId, agent.name)
        is HerdrSession.Agent.Ended -> null
    }

/** One space, as a row of the Spaces list. */
@Composable
fun SpaceRow(model: AppModel, space: AgentSpace, showMachine: Boolean, first: Boolean) {
    InboxRow(
        space.title, if (showMachine) "${space.summary} · ${space.key.machine}" else space.summary,
        Modifier.testTag("space-${space.title}"), dim = space.isEnded, first = first,
        onClick = { model.path.add(Route.Space(space.key)) },
        leading = { StatusGlyph(space.glyph) },
        trailing = { WhereTag(space.backend) },
    )
}

/** Every live space with its counts, a filter by where they live, and the ended ones below. */
@Composable
fun SpacesList(model: AppModel, machine: String? = null) {
    var filter by rememberSaveable { mutableStateOf("all") }
    val all = model.spaces(machine, since = null)
    val live = all.filter { !it.isEnded }
    val shown = live.filter { filter == "all" || it.backend == filter }
    val ended = all.filter { it.isEnded && (filter == "all" || it.backend == filter) }
    val machines = machine == null && all.map { it.key.machine }.toSet().size > 1
    Column(Modifier.fillMaxWidth()) {
        Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            listOf("all" to "All ${live.size}", "hc" to "App ${live.count { it.isHerdrApp }}", "herdr" to "Terminal ${live.count { !it.isHerdrApp }}")
                .forEach { (id, label) ->
                    Chip(label, filter == id, Modifier.clickable { filter = id }.testTag("filter-$id"))
                }
            Spacer(Modifier.weight(1f))
            if (machine == null) {
                Text(
                    "Machines", Modifier.heightIn(min = 44.dp).clickable { model.path.add(Route.Machines) }.padding(vertical = 12.dp).testTag("machinesLink"),
                    style = Type.sub.copy(fontWeight = FontWeight.Medium), color = palette.ac,
                )
            }
        }
        if (shown.isEmpty()) StateRow(if (live.isEmpty()) "No spaces are live." else "None live here.")
        shown.forEachIndexed { i, s -> SpaceRow(model, s, machines, i == 0) }
        if (ended.isNotEmpty()) {
            SectionHead("Ended", "${ended.size}")
            ended.take(30).forEachIndexed { i, s -> SpaceRow(model, s, machines, i == 0) }
        }
    }
}

/** One space: its agents, those that ended today, and where to find it at your desk. */
@Composable
fun SpaceScreen(model: AppModel, voice: Voice, key: AgentSpace.Key) {
    var renaming by remember(key) { mutableStateOf(false) }
    var starting by remember(key) { mutableStateOf(false) }
    // One request id per name tried, so a retry after a lost answer is the same request.
    val renameIds = remember(key) { mutableMapOf<String, String>() }
    LaunchedEffect(key, model.wakes, model.foreground) {
        while (model.foreground) {
            model.refreshAgents()
            delay(6_000)
        }
    }
    val space = model.spaces(key.machine, HerdrSession.startOfToday()).firstOrNull { it.key == key }
        ?: model.spaces(key.machine, null).firstOrNull { it.key == key }
    val app = key.session == HerdrSession.HERDR_APP
    InboxPage(top = { PageTop(model.backLabel, model::pop, key.machine) }) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, bottom = 32.dp)) {
            val n = space?.live?.size ?: 0
            StateLine(
                stateText("${space?.title ?: key.space ?: key.session}.", if (n == 0) "Nothing running." else "$n ${if (n == 1) "agent" else "agents"}."),
                Modifier.padding(top = 8.dp),
            )
            Row(Modifier.padding(top = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                WhereTag(space?.backend ?: if (app) "hc" else "herdr")
                if (space?.fromSwitchboard == true) Text("Started from Switchboard", style = Type.sub, color = palette.sub)
            }
            if (space != null && space.live.isNotEmpty()) {
                SectionHead("Agents", "${space.live.size}")
                space.live.forEachIndexed { i, a -> AgentRow(model, a, i == 0) }
            }
            if (space != null && space.ended.isNotEmpty()) {
                SectionHead("Ended today", "${space.ended.size}")
                space.ended.forEachIndexed { i, a -> AgentRow(model, a, i == 0) }
            }
            if (space == null || space.agents.isEmpty()) StateRow("No agents in this space.")
            if (space != null && space.newAgent == Offer.Yes) {
                InboxRow(
                    "New agent here", "A new session, in this space", Modifier.testTag("newAgentHere"), first = space.agents.isEmpty(),
                    onClick = { starting = true },
                    leading = { Icon(Icons.Filled.Add, null, Modifier.size(22.dp), tint = palette.ac) },
                    trailing = { Chevron() },
                )
            }
            SectionHead("At your desk")
            FieldRow("Where", "${if (app) "Herdr app" else "Herdr terminal"} on ${key.machine}", first = true)
            if (app) {
                FieldRow("Find it under", key.space?.let { "the space “$it”" } ?: "no space")
            } else {
                key.space?.let { FieldRow("Workspace", it) }
                FieldRow("Attach", "") { SelectionContainer { Text("herdr session attach ${key.session}", style = Type.mono(12.5f), color = palette.sub) } }
            }
            if (space != null) {
                NameRow(key.space ?: space.title, space.rename) { renaming = true }
                (space.newAgent as? Offer.No)?.let {
                    Hairline()
                    Note(it.why, Modifier.padding(vertical = 11.dp).testTag("newAgentBlocked"), palette.sub)
                }
            }
            Hairline()
        }
    }
    val id = space?.spaceId
    if (renaming && space != null && id != null) {
        val name = key.space ?: space.title
        RenameSheet("Rename $name", name, dismiss = { renaming = false }) { new ->
            val hub = model.hub ?: return@RenameSheet
            val listed = hub.renameSpace(key.machine, key.session, id, new, renameIds.getOrPut(new) { UUID.randomUUID().toString() })
            model.refreshAgents()
            model.replaceTop(Route.Space(key.copy(space = listed)))
        }
    }
    if (starting && space != null && id != null) {
        StartSheet(model, voice, space.project, StartInto(id, space.title, key.machine)) {
            starting = false
            model.refreshAgentsSoon()
        }
    }
}

@Composable
private fun AgentRow(model: AppModel, a: HerdrSession.Agent, first: Boolean) {
    val status = a.status
    InboxRow(
        a.nameInSpace, a.subtitle, Modifier.testTag("agent-${a.nameInSpace}"),
        subtitleColor = if (status.glyph == Glyph.Need) palette.warn else null, dim = a.isEnded, first = first,
        onClick = a.route?.let { r -> { model.path.add(r) } },
        leading = { StatusGlyph(if (a.isEnded) Glyph.End else status.glyph) },
        trailing = if (a.route != null) ({ Chevron() }) else null,
    )
}
