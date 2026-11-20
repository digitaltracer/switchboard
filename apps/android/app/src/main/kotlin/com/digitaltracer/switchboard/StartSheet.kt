package com.digitaltracer.switchboard

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.BackendOption
import com.digitaltracer.switchboard.api.models.Machine
import com.digitaltracer.switchboard.api.models.Project
import com.digitaltracer.switchboard.api.models.StartOptions
import com.digitaltracer.switchboard.core.HubError
import com.digitaltracer.switchboard.core.StartFlow
import com.digitaltracer.switchboard.core.StartState
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * New session: the prompt, typed or dictated; whether it opens in the Herdr app or a Herdr
 * terminal; and the project, machine and agent. A lost answer becomes "outcome unknown" and is
 * reconciled by request id; the same start within a minute comes back as "already running".
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun StartSheet(model: AppModel, voice: Voice, initialProject: String?, done: (String?) -> Unit) {
    var projects by remember { mutableStateOf<List<Project>>(emptyList()) }
    var machines by remember { mutableStateOf<List<Machine>>(emptyList()) }
    var project by remember { mutableStateOf(initialProject.orEmpty()) }
    var machine by remember { mutableStateOf("") }
    var agent by remember { mutableStateOf("claude") }
    var prompt by remember { mutableStateOf("") }
    var notify by remember { mutableStateOf(true) }
    // `hc` or `herdr`, once the machine has said what it can take.
    var backend by remember { mutableStateOf<String?>(null) }
    var options by remember { mutableStateOf<StartOptions?>(null) }
    var optionsError by remember { mutableStateOf<String?>(null) }
    // The hub predates choosing where a session opens.
    var hubTooOld by remember { mutableStateOf(false) }
    var state by remember { mutableStateOf<StartState?>(null) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var sending by remember { mutableStateOf(false) }
    var startJob by remember { mutableStateOf<Job?>(null) }
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val askMic = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) voice.dictate(prompt) else voice.denied()
    }
    val canChoose = options?.default != null

    fun takeDictation() {
        voice.text.trim().takeIf { it.isNotEmpty() }?.let { prompt = it }
        voice.finish()
    }

    LaunchedEffect(Unit) {
        val hub = model.hub ?: return@LaunchedEffect
        try {
            projects = hub.projects()
            machines = hub.machines()
            val p = projects.firstOrNull { it.id == project } ?: projects.firstOrNull()
            p?.let {
                project = it.id
                agent = it.defaultAgent
                val preferred = machines.firstOrNull { m -> m.default }?.id ?: machines.firstOrNull()?.id.orEmpty()
                machine = if (preferred in it.machines) preferred else it.machines.firstOrNull() ?: preferred
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            loadError = e.message
        }
    }
    // Asks the chosen machine where a session would open; picks that, or the other if it can't.
    LaunchedEffect(machine) {
        options = null
        optionsError = null
        hubTooOld = false
        backend = null
        val hub = model.hub ?: return@LaunchedEffect
        if (machine.isEmpty()) return@LaunchedEffect
        try {
            val o = hub.startOptions(machine)
            options = o
            val available = o.backends.filter { it.available }.map { it.backend }
            backend = o.default?.takeIf { it in available } ?: available.firstOrNull()
        } catch (e: CancellationException) {
            throw e
        } catch (e: HubError) {
            if (e.code == "http_404") hubTooOld = true else optionsError = e.message
        } catch (e: Exception) {
            optionsError = e.message
        }
    }
    // Dictation ends in review; its words come here, where they stay editable.
    LaunchedEffect(voice.phase, voice.retrying, voice.canRetry) {
        if (voice.phase == Voice.Phase.Review && voice.dictating && !voice.retrying && !voice.canRetry) takeDictation()
    }

    fun start() {
        val hub = model.hub ?: return
        startJob = scope.launch {
            sending = true
            val requestId = UUID.randomUUID().toString()
            state = StartFlow.run(hub, project, prompt, machine.ifEmpty { null }, agent, notify, requestId, if (canChoose) backend else null)
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
        voice.cancel()
        done(null)
    }
    ModalBottomSheet(
        onDismissRequest = { if (!voice.dictating) cancel() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = palette.bg,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
    ) {
        Box(Modifier.fillMaxHeight(0.95f)) {
            Column(Modifier.imePadding().testTag("newSession")) {
                Row(Modifier.fillMaxWidth().height(44.dp).padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("Cancel", Modifier.clickable(onClick = cancel).padding(vertical = 10.dp), style = Type.field.copy(fontWeight = FontWeight.Medium), color = palette.ac)
                    Spacer(Modifier.weight(1f))
                    Text("New session", style = Type.meta, color = palette.mut)
                }
                Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).padding(bottom = 16.dp)) {
                    StateLine(stateText("Start something new."), Modifier.padding(top = 8.dp))
                    loadError?.let { Note(it, Modifier.padding(top = 10.dp), palette.bad) }
                    Note("What should the agent do?", Modifier.padding(top = 14.dp, bottom = 8.dp))
                    Box(
                        Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(palette.sf)
                            .border(1.dp, palette.line, RoundedCornerShape(14.dp)),
                    ) {
                        BasicTextField(
                            prompt, { prompt = it },
                            Modifier.fillMaxWidth().heightIn(min = 96.dp).padding(start = 16.dp, end = 60.dp, top = 14.dp, bottom = 14.dp).testTag("promptField"),
                            textStyle = Type.say.copy(color = palette.ink),
                            cursorBrush = SolidColor(palette.ac),
                            decorationBox = { inner ->
                                if (prompt.isEmpty()) Text("Fix the flaky retry test and tell me why it failed", style = Type.say, color = palette.mut)
                                inner()
                            },
                        )
                        Box(Modifier.align(Alignment.BottomEnd).padding(10.dp)) {
                            MicDisc(40.dp, "Dictate the prompt", "dictateButton") {
                                if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
                                    voice.dictate(prompt)
                                } else {
                                    askMic.launch(Manifest.permission.RECORD_AUDIO)
                                }
                            }
                        }
                    }
                    // A dictation that failed waits here for Retry; its words so far stay.
                    if (voice.dictating && voice.phase == Voice.Phase.Review) {
                        Column(Modifier.padding(top = 10.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            voice.error?.let { Note(it, color = palette.warn) }
                            if (voice.retrying) {
                                Row(Modifier.testTag("voiceRetrying"), verticalAlignment = Alignment.CenterVertically) {
                                    CircularProgressIndicator(Modifier.size(16.dp), color = palette.mut, strokeWidth = 2.dp)
                                    Note("Transcribing with OpenAI…", Modifier.padding(start = 8.dp))
                                }
                            } else {
                                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                                    if (voice.canRetry) BigButton("Retry", Modifier.weight(1f).testTag("voiceRetry"), ButtonKind.Outline) { voice.retry() }
                                    BigButton("Keep what I have", Modifier.weight(1f), ButtonKind.Plain, color = palette.sub) { takeDictation() }
                                }
                            }
                        }
                    }
                    SectionHead("Open it in", top = 16.dp)
                    listOf("hc", "herdr").forEachIndexed { i, b ->
                        val opt = options?.backends?.firstOrNull { it.backend == b }
                        val offered = canChoose && opt?.available == true
                        RadioRow(
                            if (b == "hc") "Herdr app" else "Herdr terminal", canChoose && backend == b, Modifier.testTag("backend-$b"),
                            offered = offered || (options == null && optionsError == null && !hubTooOld), first = i == 0,
                            onClick = if (offered) ({ backend = b }) else null,
                        ) { Note(describe(b, machine, options, opt, optionsError, hubTooOld)) }
                    }
                    Hairline()
                    SectionHead("What runs", top = 16.dp)
                    Field("Project", project.ifEmpty { "Choose" }, projects.map { it.id to it.id }, "projectPicker") { id ->
                        projects.firstOrNull { it.id == id }?.let { p ->
                            project = p.id
                            agent = p.defaultAgent
                            if (machine !in p.machines) machine = p.machines.firstOrNull() ?: machine
                        }
                    }
                    val available = projects.firstOrNull { it.id == project }?.machines
                    val status = machines.firstOrNull { it.id == machine }?.status
                    Field(
                        "Machine", if (machine.isEmpty()) "Choose" else "$machine · ${status ?: "unknown"}",
                        machines.filter { available?.contains(it.id) ?: true }.map { it.id to "${it.id} · ${it.status}" }, "machinePicker",
                        dot = if (machine.isEmpty()) null else status == "ready",
                    ) { machine = it }
                    Field("Agent", agentName(agent) ?: agent, listOf("claude" to "Claude Code", "codex" to "Codex"), "agentPicker") { agent = it }
                    Column {
                        Hairline()
                        Row(Modifier.fillMaxWidth().heightIn(min = 50.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text("Notify me when it needs me", Modifier.weight(1f), style = Type.field, color = palette.ink)
                            Switch(
                                notify, { notify = it },
                                colors = SwitchDefaults.colors(checkedTrackColor = palette.ink, checkedThumbColor = palette.inv, uncheckedTrackColor = palette.fill),
                            )
                        }
                        Hairline()
                    }
                    state?.let { Box(Modifier.padding(top = 14.dp)) { StartStatus(it) { id -> done(id) } } }
                }
                BigButton(
                    "Start", Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 8.dp).testTag("submitStart"),
                    enabled = project.isNotEmpty() && prompt.isNotBlank() && !sending && !voice.dictating, busy = sending,
                ) { start() }
            }
            if (voice.dictating) VoiceOverlay(voice)
        }
    }
}

private fun describe(b: String, machine: String, options: StartOptions?, o: BackendOption?, error: String?, hubTooOld: Boolean): String {
    val m = machine.ifEmpty { "the machine" }
    if (hubTooOld) return if (b == "hc") "Your hub is too old to choose. The session opens where $m is set to." else "Update the hub to choose."
    if (error != null) return "Couldn't ask $m: $error"
    if (options == null || o == null) return "Checking $m…"
    if (options.default == null) {
        return if (b == "hc") "$m’s runner is too old to choose. The session opens where it’s set to." else "Update the runner on $m to choose."
    }
    if (o.available) {
        return if (b == "hc") "A new space in the Herdr window on $m, on its own branch. The app is open there now."
        else "Its own Herdr session. Attach to it from any terminal on $m."
    }
    return when (o.reason) {
        "not_answering" -> "The Herdr app isn’t open on $m. Open it there to start here."
        "not_followed" -> "$m’s runner doesn’t use the Herdr app."
        else -> "$m can’t open it here now."
    }
}

/** A field that opens a menu of choices. */
@Composable
private fun Field(label: String, value: String, choices: List<Pair<String, String>>, tag: String, dot: Boolean? = null, pick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        Column(Modifier.fillMaxWidth().clickable { open = true }.testTag(tag)) {
            Hairline()
            Row(Modifier.fillMaxWidth().heightIn(min = 50.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(label, Modifier.weight(1f), style = Type.field, color = palette.ink)
                dot?.let { Dot(if (it) palette.ok else palette.warn); Spacer(Modifier.width(8.dp)) }
                Text(value, style = Type.field, color = palette.sub, maxLines = 1)
                Spacer(Modifier.width(8.dp))
                Chevron(down = true)
            }
        }
        DropdownMenu(open, { open = false }, Modifier.align(Alignment.TopEnd)) {
            choices.forEach { (id, text) ->
                DropdownMenuItem({ Text(text, style = Type.field) }, {
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
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Note("Already running: the same start was sent in the last minute.", color = palette.sub)
                BigButton("Open it", Modifier.fillMaxWidth().testTag("openDuplicate"), ButtonKind.Outline) { open(state.sessionId) }
            }
        } else {
            Note("Started.", color = palette.ok)
        }
        is StartState.Unknown -> Row(Modifier.testTag("reconciling"), verticalAlignment = Alignment.CenterVertically) {
            CircularProgressIndicator(Modifier.size(16.dp), color = palette.mut, strokeWidth = 2.dp)
            Note("Outcome unknown; checking with the hub…", Modifier.padding(start = 8.dp), palette.sub)
        }
        is StartState.Failed -> Note(state.message, Modifier.testTag("startFailed"), palette.bad)
    }
}
