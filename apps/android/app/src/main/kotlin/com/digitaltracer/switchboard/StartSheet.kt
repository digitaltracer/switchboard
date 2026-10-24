package com.digitaltracer.switchboard

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.UnfoldMore
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.Icon
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Machine
import com.digitaltracer.switchboard.api.models.Project
import com.digitaltracer.switchboard.core.StartFlow
import com.digitaltracer.switchboard.core.StartState
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * Starts a session. A lost answer becomes "outcome unknown" and is reconciled by request id;
 * the same start within a minute comes back as "already running".
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun StartSheet(model: AppModel, done: (String?) -> Unit) {
    var projects by remember { mutableStateOf<List<Project>>(emptyList()) }
    var machines by remember { mutableStateOf<List<Machine>>(emptyList()) }
    var project by remember { mutableStateOf("") }
    var machine by remember { mutableStateOf("") }
    var agent by remember { mutableStateOf("claude") }
    var prompt by remember { mutableStateOf("") }
    var notify by remember { mutableStateOf(true) }
    var state by remember { mutableStateOf<StartState?>(null) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var sending by remember { mutableStateOf(false) }
    var startJob by remember { mutableStateOf<Job?>(null) }
    val scope = rememberCoroutineScope()

    LaunchedEffect(Unit) {
        val hub = model.hub ?: return@LaunchedEffect
        try {
            projects = hub.projects()
            machines = hub.machines()
            projects.firstOrNull()?.let { pick(it, machines, machine) { p, a, m -> project = p; agent = a; machine = m } }
            machine = machines.firstOrNull { it.default }?.id ?: machines.firstOrNull()?.id.orEmpty()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            loadError = e.message
        }
    }

    fun start() {
        val hub = model.hub ?: return
        startJob = scope.launch {
            sending = true
            val requestId = UUID.randomUUID().toString()
            state = StartFlow.run(hub, project, prompt, machine.ifEmpty { null }, agent, notify, requestId)
            // Reconcile an unknown outcome by asking the hub about the request, for up to five minutes.
            var tries = 0
            while (state is StartState.Unknown && tries < 150 && isActive) {
                delay(2_000)
                state = StartFlow.reconcile(hub, requestId, tries)
                tries++
            }
            sending = false
            (state as? StartState.Started)?.takeIf { !it.alreadyRunning }?.let { done(it.sessionId) }
        }
    }

    val cancel = {
        // A start still reconciling stops being watched; the hub still settles it.
        startJob?.cancel()
        done(null)
    }
    ModalBottomSheet(
        onDismissRequest = cancel,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = palette.background,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
    ) {
        Column(Modifier.fillMaxHeight(0.92f).imePadding()) {
            Box(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 12.dp)) {
                Text("Cancel", Modifier.align(Alignment.CenterStart).clickable(onClick = cancel), style = Type.body, color = palette.accent)
                Text("New session", Modifier.align(Alignment.Center), style = Type.headline, color = palette.label)
                val ready = project.isNotEmpty() && prompt.isNotBlank() && !sending
                Text(
                    "Start", Modifier.align(Alignment.CenterEnd).clickable(enabled = ready) { start() }.testTag("submitStart"),
                    style = Type.headline, color = if (ready) palette.accent else palette.tertiary,
                )
            }
            Column(
                Modifier.verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp),
                verticalArrangement = Arrangement.spacedBy(24.dp),
            ) {
                loadError?.let { Text(it, style = Type.body, color = palette.red) }
                Column(Modifier.fillMaxWidth().card(12.dp, palette.card)) {
                    Picker("Project", project, projects.map { it.id to it.id }, true, "projectPicker") { id ->
                        projects.firstOrNull { it.id == id }?.let { pick(it, machines, machine) { p, a, m -> project = p; agent = a; machine = m } }
                    }
                    val available = projects.firstOrNull { it.id == project }?.machines
                    Picker("Machine", machine, machines.filter { available?.contains(it.id) ?: true }.map { it.id to "${it.id} (${it.status})" }, false) {
                        machine = it
                    }
                    Picker("Agent", agent, listOf("claude" to "Claude Code", "codex" to "Codex"), false) { agent = it }
                }
                InsetGroup("Prompt") {
                    GroupRow(true) {
                        BasicTextField(
                            prompt, { prompt = it },
                            Modifier.fillMaxWidth().heightIn(min = 88.dp).testTag("promptField"),
                            textStyle = Type.body.copy(color = palette.label),
                            cursorBrush = SolidColor(palette.accent),
                            decorationBox = { inner ->
                                if (prompt.isEmpty()) Text("What should the agent do?", style = Type.body, color = palette.tertiary)
                                inner()
                            },
                        )
                    }
                }
                Column(Modifier.fillMaxWidth().card(12.dp, palette.card)) {
                    GroupRow(true) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text("Notify me", Modifier.weight(1f), style = Type.body, color = palette.label)
                            Switch(notify, { notify = it }, colors = SwitchDefaults.colors(checkedTrackColor = Color(0xFF34C759), checkedThumbColor = Color.White))
                        }
                    }
                }
                state?.let { StartStatus(it) { id -> done(id) } }
            }
        }
    }
}

/** A project's own agent and a machine it can run on, keeping the current one when it can. */
private fun pick(p: Project, machines: List<Machine>, current: String, set: (String, String, String) -> Unit) {
    val available = p.machines
    set(p.id, p.defaultAgent, if (current in available) current else available.firstOrNull() ?: machines.firstOrNull()?.id.orEmpty())
}

/** A form row that opens a menu of choices, as iOS's Picker does. */
@Composable
private fun Picker(label: String, value: String, choices: List<Pair<String, String>>, first: Boolean, tag: String? = null, pick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        Column(Modifier.fillMaxWidth().clickable { open = true }.then(if (tag != null) Modifier.testTag(tag) else Modifier)) {
            GroupRow(first) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(label, Modifier.weight(1f), style = Type.body, color = palette.label)
                    Text(choices.firstOrNull { it.first == value }?.second ?: value, style = Type.body, color = palette.secondary)
                    Icon(Icons.Filled.UnfoldMore, null, Modifier.padding(start = 4.dp).size(18.dp), tint = palette.secondary)
                }
            }
        }
        DropdownMenu(open, { open = false }, Modifier.align(Alignment.TopEnd)) {
            choices.forEach { (id, text) ->
                DropdownMenuItem({ Text(text, style = Type.body) }, {
                    open = false
                    pick(id)
                })
            }
        }
    }
}

@Composable
private fun StartStatus(state: StartState, open: (String) -> Unit) {
    when (state) {
        is StartState.Started -> if (state.alreadyRunning) {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("Already running: the same start was sent in the last minute.", style = Type.body, color = palette.label)
                Text("Open it", Modifier.clickable { open(state.sessionId) }, style = Type.body, color = palette.accent)
            }
        } else {
            Text("Started.", style = Type.body, color = palette.label)
        }
        is StartState.Unknown -> Text("Outcome unknown; reconciling with the hub…", Modifier.testTag("reconciling"), style = Type.body, color = palette.secondary)
        is StartState.Failed -> Text(state.message, style = Type.body, color = palette.red)
    }
}
