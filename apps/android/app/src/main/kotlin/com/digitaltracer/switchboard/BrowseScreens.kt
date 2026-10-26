package com.digitaltracer.switchboard

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.outlined.Computer
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Machine
import com.digitaltracer.switchboard.api.models.Project
import com.digitaltracer.switchboard.api.models.Session
import com.digitaltracer.switchboard.core.displayName
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/** An iOS grouped list: a large title, then sections of rows in rounded cards. */
@Composable
@OptIn(ExperimentalMaterial3Api::class)
fun GroupedList(
    model: AppModel,
    title: String,
    onRefresh: (suspend () -> Unit)? = null,
    content: androidx.compose.foundation.lazy.LazyListScope.() -> Unit,
) {
    val scope = rememberCoroutineScope()
    var refreshing by remember { mutableStateOf(false) }
    Page(model.backLabel, model::pop) {
        PullToRefreshBox(
            refreshing,
            {
                onRefresh?.let { refresh ->
                    scope.launch {
                        refreshing = true
                        refresh()
                        refreshing = false
                    }
                }
            },
            Modifier.fillMaxSize(),
        ) {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 40.dp),
                verticalArrangement = Arrangement.spacedBy(24.dp),
            ) {
                item { LargeTitle(title, horizontal = 0) }
                content()
            }
        }
    }
}

/** A row in a grouped list, with a separator above all but the first. */
@Composable
fun GroupRow(first: Boolean, onClick: (() -> Unit)? = null, content: @Composable () -> Unit) {
    Column(Modifier.fillMaxWidth().then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier).padding(start = 16.dp)) {
        if (!first) HorizontalDivider(thickness = 0.5.dp, color = palette.separator)
        Row(Modifier.heightIn(min = 44.dp).padding(top = 10.dp, bottom = 10.dp, end = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) { content() }
            if (onClick != null) Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, null, Modifier.size(22.dp), tint = palette.tertiary)
        }
    }
}

/**
 * Every session the hub knows, running first, then the agents started in Herdr; or just one
 * machine's.
 */
@Composable
fun AllSessionsScreen(model: AppModel, machine: String? = null) {
    LaunchedEffect(machine) { model.refreshAgents() }
    val sessions = model.sessions.filter { machine == null || it.machineId == machine }
    val agents = model.agents.filter { machine == null || it.machineId == machine }
    val live = sessions.filter { it.live }
    val ended = sessions.filter { !it.live }
    GroupedList(model, machine ?: "All sessions", onRefresh = model::refreshAgents) {
        if (live.isNotEmpty()) item { InsetGroup("Running") { live.forEachIndexed { i, s -> SessionRow(model, s, i == 0) } } }
        if (agents.isNotEmpty()) {
            item {
                InsetGroup("Started in Herdr") {
                    agents.forEachIndexed { i, a ->
                        GroupRow(i == 0, { model.path.add(Route.AgentPage(a.machineId, a.name)) }) {
                            Text(a.displayName, style = Type.headline, color = palette.label)
                            val status = SessionStatus.agent(a.agentStatus)
                            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                Text(status.text, style = Type.subheadline, color = status.color)
                                Text("· ${a.herdrSession ?: a.machineId}", style = Type.subheadline, color = palette.secondary)
                            }
                        }
                    }
                }
            }
        }
        if (ended.isNotEmpty()) item { InsetGroup("Ended") { ended.forEachIndexed { i, s -> SessionRow(model, s, i == 0) } } }
        if (sessions.isEmpty() && agents.isEmpty()) item { Text("No sessions yet.", style = Type.body, color = palette.secondary) }
    }
}

@Composable
private fun SessionRow(model: AppModel, s: Session, first: Boolean) {
    GroupRow(first, { model.path.add(Route.SessionPage(s.id)) }) {
        Row {
            Text(s.label, Modifier.weight(1f), style = Type.headline, color = palette.label, maxLines = 1)
            Text(Ago.short(s.updatedAt), style = Type.subheadline, color = palette.secondary)
        }
        val status = SessionStatus.of(s)
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(status.text, style = Type.subheadline, color = status.color)
            Text("· ${s.projectId} · ${s.machineId}", style = Type.subheadline, color = palette.secondary, maxLines = 1)
        }
    }
}

@Composable
fun MachinesScreen(model: AppModel) {
    var machines by remember { mutableStateOf<List<Machine>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    suspend fun load() {
        try {
            machines = model.hub?.machines().orEmpty()
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message
        }
        model.refreshAgents()
    }
    LaunchedEffect(Unit) { load() }
    GroupedList(model, "Machines", onRefresh = ::load) {
        error?.let { item { Text(it, style = Type.body, color = palette.red) } }
        if (machines.isNotEmpty()) {
            item {
                Column(Modifier.fillMaxWidth().card(12.dp, palette.card)) {
                    machines.forEachIndexed { i, m ->
                        GroupRow(i == 0, { model.path.add(Route.MachinePage(m.id)) }) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Icon(Icons.Outlined.Computer, null, Modifier.size(22.dp), tint = palette.accent)
                                Spacer(Modifier.width(12.dp))
                                Column(Modifier.weight(1f)) {
                                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                                        Text(m.id, style = Type.headline, color = palette.label)
                                        if (m.default) Text("default", style = Type.caption, color = palette.secondary)
                                    }
                                    Text(counts(model, m.id), style = Type.subheadline, color = palette.secondary)
                                }
                                Text(m.status, style = Type.subheadline, color = if (m.status == "ready") palette.done else palette.secondary)
                            }
                        }
                    }
                }
            }
        }
    }
}

/** "1 session running · 6 agents in Herdr, 5 idle" */
private fun counts(model: AppModel, machine: String): String {
    val running = model.sessions.count { it.machineId == machine && it.lifecycle == "running" }
    val agents = model.agents.filter { it.machineId == machine }
    var text = "$running ${if (running == 1) "session" else "sessions"} running"
    if (agents.isEmpty()) return text
    text += " · ${agents.size} ${if (agents.size == 1) "agent" else "agents"} in Herdr"
    val idle = agents.count { it.agentStatus == "idle" || it.agentStatus == "done" }
    if (idle == agents.size) text += if (agents.size == 1) ", idle" else ", all idle" else if (idle > 0) text += ", $idle idle"
    return text
}

@Composable
fun ProjectsScreen(model: AppModel) {
    var projects by remember { mutableStateOf<List<Project>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    suspend fun load() {
        try {
            projects = model.hub?.projects().orEmpty()
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message
        }
    }
    LaunchedEffect(Unit) { load() }
    GroupedList(model, "Projects", onRefresh = ::load) {
        error?.let { item { Text(it, style = Type.body, color = palette.red) } }
        if (projects.isNotEmpty()) {
            item {
                Column(Modifier.fillMaxWidth().card(12.dp, palette.card)) {
                    projects.forEachIndexed { i, p ->
                        GroupRow(i == 0) {
                            Row {
                                Text(p.id, Modifier.weight(1f), style = Type.headline, color = palette.label)
                                val n = model.sessions.count { it.projectId == p.id && it.lifecycle == "running" }
                                if (n > 0) Text("$n running", style = Type.subheadline, color = palette.working)
                            }
                            if (p.description.isNotEmpty()) Text(p.description, style = Type.subheadline, color = palette.secondary)
                            Text("${p.defaultAgent.capitalized()} · ${p.machines.joinToString(", ")}", style = Type.footnote, color = palette.secondary)
                        }
                    }
                }
            }
        }
    }
}
