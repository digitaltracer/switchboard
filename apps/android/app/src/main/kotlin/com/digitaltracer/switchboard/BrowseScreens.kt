package com.digitaltracer.switchboard

import androidx.compose.foundation.background
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Machine
import com.digitaltracer.switchboard.api.models.Project
import com.digitaltracer.switchboard.core.agentStatus
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
fun GroupRow(first: Boolean, onClick: (() -> Unit)? = null, background: Color = Color.Transparent, content: @Composable () -> Unit) {
    Column(Modifier.fillMaxWidth().background(background).then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier).padding(start = 16.dp)) {
        if (!first) HorizontalDivider(thickness = 0.5.dp, color = palette.separator)
        Row(Modifier.heightIn(min = 44.dp).padding(top = 10.dp, bottom = 10.dp, end = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) { content() }
            if (onClick != null) Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, null, Modifier.size(22.dp), tint = palette.tertiary)
        }
    }
}

/** Every Herdr session with its agents, running first and ended at the bottom; or just one machine's. */
@Composable
fun AllSessionsScreen(model: AppModel, machine: String? = null) {
    LaunchedEffect(machine) { model.refreshAgents() }
    val groups = model.herdrSessions(machine, since = null)
    GroupedList(model, machine ?: "All sessions", onRefresh = model::refreshAgents) {
        if (groups.isNotEmpty()) {
            item {
                Column(Modifier.fillMaxWidth().card(12.dp, palette.card)) {
                    groups.forEachIndexed { i, g -> SessionRow(model, g, machine == null, i == 0) }
                }
            }
        } else {
            item { Text("No sessions yet.", style = Type.body, color = palette.secondary) }
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

/** "2 sessions · 5 agents, 1 working" */
private fun counts(model: AppModel, machine: String): String {
    val live = model.herdrSessions(machine, since = null).filter { !it.isEnded }
    val agents = live.flatMap { it.live }
    var text = "${live.size} ${if (live.size == 1) "session" else "sessions"}"
    if (agents.isEmpty()) return text
    text += " · ${agents.size} ${if (agents.size == 1) "agent" else "agents"}"
    val working = agents.count { it.agentStatus == "working" }
    text += if (working > 0) ", $working working" else if (agents.size == 1) ", idle" else ", all idle"
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
