package com.digitaltracer.switchboard

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Machine
import com.digitaltracer.switchboard.core.agentStatus
import kotlin.coroutines.cancellation.CancellationException

/** One machine's spaces. */
@Composable
fun MachineSpacesScreen(model: AppModel, machine: String) {
    LaunchedEffect(machine) { model.refreshAgents() }
    val live = model.spaces(machine, null).count { !it.isEnded }
    InboxPage(top = { PageTop(model.backLabel, model::pop) }) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, bottom = 32.dp)) {
            StateLine(stateText("$machine.", if (live == 0) "Nothing live." else "$live ${if (live == 1) "space" else "spaces"} live."), Modifier.padding(top = 8.dp, bottom = 14.dp))
            SpacesList(model, machine)
        }
    }
}

/** Every machine with its readiness and what runs there. A session always stays on the machine it started on. */
@Composable
fun MachinesScreen(model: AppModel) {
    var machines by remember { mutableStateOf<List<Machine>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        val hub = model.hub ?: return@LaunchedEffect
        try {
            machines = hub.machines()
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message
        }
        model.refreshAgents()
    }
    InboxPage(top = { PageTop(model.backLabel, model::pop) }) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, bottom = 32.dp)) {
            val list = machines
            val headline = if (list == null) stateText("Machines.") else {
                val ready = list.count { it.status == "ready" }
                val n = list.size
                stateText("$n ${if (n == 1) "machine" else "machines"}.", if (ready == n) (if (n == 1) "It's ready." else if (n == 2) "Both ready." else "All ready.") else "${n - ready} not ready.")
            }
            StateLine(headline, Modifier.padding(top = 8.dp))
            Text("Choose one to see only its spaces.", Modifier.padding(top = 6.dp), style = Type.text, color = palette.sub)
            Column(Modifier.padding(top = 20.dp)) {
                error?.let { StateRow("Couldn't list the machines: $it. Go back and try again.", error = true) }
                if (list == null && error == null) StateRow("Listing machines…", busy = true)
                list.orEmpty().forEachIndexed { i, m ->
                    InboxRow(
                        m.id, counts(model, m.id), Modifier.testTag("machine-${m.id}"), first = i == 0,
                        onClick = { model.path.add(Route.MachinePage(m.id)) },
                        trailing = {
                            if (m.default) Chip("default")
                            Row(Modifier.padding(start = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                Dot(if (m.status == "ready") palette.ok else palette.warn)
                                Text(m.status, style = Type.meta.copy(fontWeight = FontWeight.Medium), color = if (m.status == "ready") palette.ok else palette.warn)
                            }
                            Chevron()
                        },
                    )
                }
                Hairline()
            }
            Note("A session always stays on the machine it started on.", Modifier.padding(top = 16.dp))
        }
    }
}

/** "2 spaces · 13 agents, 3 working" */
private fun counts(model: AppModel, machine: String): String {
    val live = model.spaces(machine, null).filter { !it.isEnded }
    if (live.isEmpty()) return "No spaces right now"
    val agents = live.flatMap { it.live }
    val working = agents.count { it.agentStatus == "working" }
    return "${live.size} ${if (live.size == 1) "space" else "spaces"} · ${agents.size} ${if (agents.size == 1) "agent" else "agents"}" +
        if (working > 0) ", $working working" else if (agents.size == 1) ", idle" else ", all idle"
}

/** The allowlisted projects; tapping one starts a session there. */
@Composable
fun ProjectsList(model: AppModel, start: (String) -> Unit) {
    Column(Modifier.fillMaxWidth()) {
        if (model.projects.isEmpty()) StateRow("Listing projects…", busy = true)
        model.projects.forEachIndexed { i, p ->
            val running = model.sessions.count { it.projectId == p.id && it.lifecycle == "running" }
            Column(Modifier.fillMaxWidth().clickable { start(p.id) }.testTag("project-${p.id}")) {
                if (i > 0) Hairline()
                Row(Modifier.fillMaxWidth().heightIn(min = 60.dp).padding(vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                        Text(p.id, style = Type.title, color = palette.ink)
                        if (p.description.isNotEmpty()) Text(p.description, style = Type.sub, color = palette.sub, maxLines = 1)
                        Text("${agentName(p.defaultAgent)} · ${p.machines.joinToString(", ")}", style = Type.sub, color = palette.sub, maxLines = 1)
                    }
                    if (running > 0) Chip("$running running") else Chevron()
                }
            }
        }
        Hairline()
        Note("Only projects on the hub’s allowlist appear here.", Modifier.padding(top = 14.dp))
    }
}
