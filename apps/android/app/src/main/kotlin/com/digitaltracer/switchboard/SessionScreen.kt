package com.digitaltracer.switchboard

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material.icons.outlined.Notifications
import androidx.compose.material.icons.outlined.NotificationsOff
import androidx.compose.material.icons.outlined.StopCircle
import androidx.compose.material.icons.outlined.Warning
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Changes
import com.digitaltracer.switchboard.api.models.Session
import com.digitaltracer.switchboard.core.sessionTarget
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.sp
import com.digitaltracer.switchboard.api.models.DeleteResponse

/**
 * One session, read like a message: its state, what you said, what it's doing, what it wants
 * and what it changed. Long-polled while on screen.
 */
@Composable
fun SessionScreen(model: AppModel, voice: Voice, listen: Listen, sessionId: String) {
    var session by remember { mutableStateOf<Session?>(null) }
    var output by remember { mutableStateOf("") }
    var outputError by remember { mutableStateOf<String?>(null) }
    var changes by remember { mutableStateOf<Changes?>(null) }
    // From the last action the user took; polls don't clear it.
    var error by remember { mutableStateOf<String?>(null) }
    // The long-poll's own trouble, cleared once it gets through again.
    var pollError by remember { mutableStateOf<String?>(null) }
    // What a delete did, kept on screen since the session leaves the lists.
    var notice by remember { mutableStateOf<String?>(null) }
    var confirmStop by remember { mutableStateOf(false) }
    var showingOutput by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val list = rememberLazyListState()

    suspend fun loadOutput() {
        val hub = model.hub ?: return
        try {
            output = hub.output(sessionId, lines = 300).text
            outputError = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            outputError = "Couldn't refresh the output: ${e.message}"
        }
        // Older runners don't list changes; the section just stays hidden.
        runCatching { hub.changes(sessionId) }.onSuccess { changes = it }
    }

    suspend fun refresh() {
        val hub = model.hub ?: return
        try {
            session = hub.session(sessionId)
            loadOutput()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message
        }
    }

    fun act(call: suspend () -> Unit) {
        scope.launch {
            try {
                call()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = e.message
            }
            refresh()
        }
    }

    // Long-polls until the screen goes away; each change also refreshes the output and changes.
    LaunchedEffect(sessionId, model.wakes) {
        pollError = null
        var revision: Long? = null
        while (true) {
            val hub = model.hub ?: return@LaunchedEffect
            try {
                val s = hub.session(sessionId, after = revision)
                val changed = s.revision != revision
                session = s
                revision = s.revision
                pollError = null
                if (changed) loadOutput()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                pollError = e.message
                delay(3_000)
            }
        }
    }
    // The agent writes without changing the session's status, so the tail is read every few
    // seconds while it runs and the screen is open.
    LaunchedEffect(sessionId, model.wakes) {
        while (true) {
            delay(3_000)
            if (session?.lifecycle == "running") loadOutput()
        }
    }

    val s = session
    Page(model.backLabel, model::pop, bottomBar = s?.let { current ->
        {
            val running = current.lifecycle == "running"
            val target = sessionTarget(current)
            Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Row(Modifier.weight(1f), horizontalArrangement = Arrangement.SpaceEvenly) {
                    BarIcon(
                        if (running) Icons.Outlined.StopCircle else Icons.Outlined.Delete,
                        if (running) "Stop or delete session" else "Delete session",
                        tag = "stopOrDelete",
                        enabled = current.lifecycle !in setOf("starting", "deleted"),
                        tint = palette.red,
                    ) { confirmStop = true }
                    BarIcon(
                        if (current.muted) Icons.Outlined.NotificationsOff else Icons.Outlined.Notifications,
                        if (current.muted) "Turn notifications on" else "Mute notifications",
                    ) { act { model.hub?.mute(sessionId, !current.muted) } }
                }
                MicButton(voice, 60.dp, "Talk to ${current.label}", "sessionMic", enabled = running) { listen(target, false) }
                Row(Modifier.weight(1f), horizontalArrangement = Arrangement.SpaceEvenly) {
                    BarIcon(Icons.Outlined.Description, "Changes", enabled = changes?.files?.isNotEmpty() == true) {
                        scope.launch { list.animateScrollToItem(list.layoutInfo.totalItemsCount - 2) }
                    }
                    BarIcon(Icons.Outlined.Keyboard, "Type a message to ${current.label}", enabled = running) { voice.type(target) }
                }
            }
        }
    }) {
        if (s == null) {
            Box(Modifier.fillMaxSize(), Alignment.Center) { CircularProgressIndicator(color = palette.secondary) }
            return@Page
        }
        LazyColumn(
            Modifier.fillMaxSize(),
            state = list,
            contentPadding = PaddingValues(start = 20.dp, end = 20.dp, bottom = 40.dp),
            verticalArrangement = Arrangement.spacedBy(18.dp),
        ) {
            item {
                Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    LargeTitle(s.label, horizontal = 0)
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        StatusPill(SessionStatus.of(s))
                        Text(
                            listOfNotNull(s.agent.capitalized(), s.machineId, s.branch).joinToString(" · "),
                            Modifier.weight(1f, fill = false), style = Type.subheadline, color = palette.secondary, maxLines = 1,
                        )
                        if (s.muted) Icon(Icons.Outlined.NotificationsOff, "Muted", Modifier.size(16.dp), tint = palette.secondary)
                    }
                }
            }
            error?.let { item { Text(it, style = Type.body, color = palette.red) } }
            notice?.let { item { Text(it, Modifier.testTag("deleteNotice"), style = Type.footnote, color = palette.secondary) } }
            pollError?.let { item { Text(it, style = Type.footnote, color = palette.needs) } }
            s.error?.let { item { Text(it, style = Type.footnote, color = palette.red) } }
            if (s.hookFailed) {
                item {
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Icon(Icons.Outlined.Warning, null, Modifier.size(16.dp), tint = palette.red)
                        Text("It's stuck at a dialog Switchboard won't answer. Answer it in Herdr.", style = Type.footnote, color = palette.red)
                    }
                }
            }
            model.said[s.id]?.let { item { SaidBubble(it) } }
            items(s.pendingApprovals, key = { it.id }) { a -> ApprovalCard(model, a, s, decided = { refresh() }) }
            item {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("LATEST", style = Type.footnote, fontWeight = FontWeight.SemiBold, color = palette.secondary)
                    OutputBlock(output)
                    outputError?.let { Text(it, style = Type.footnote, color = palette.needs) }
                    Row(Modifier.clickable { showingOutput = true }.padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text("Show the full output", style = Type.body, color = palette.accent)
                        Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, null, Modifier.size(18.dp), tint = palette.accent)
                    }
                }
            }
            changes?.takeIf { it.files.isNotEmpty() }?.let { item { ChangesGrid(it) } }
            item { Details(s) }
        }
    }

    if (showingOutput) FullOutput(s?.label ?: "Output", output) { showingOutput = false }
    if (confirmStop) {
        val running = s?.lifecycle == "running"
        ActionSheet(
            if (running) {
                "Stop keeps the worktree and branch. Delete also deletes its Herdr session, and its worktree if it's clean and merged."
            } else {
                "Delete this session? Its Herdr session is deleted, and its worktree if it's clean and merged. The branch is kept."
            },
            listOfNotNull(
                ("Stop" to { act { model.hub?.stop(sessionId) } }).takeIf { running },
                "Delete" to {
                    act {
                        model.hub?.let { notice = describe(it.delete(sessionId)) }
                    }
                },
            ),
        ) { confirmStop = false }
    }
}

private fun describe(r: DeleteResponse): String {
    val branch = r.branch?.let { " Branch $it is kept." }.orEmpty()
    return when {
        r.alreadyDeleted -> "Already deleted."
        r.worktreeRemoved -> "Deleted, with its worktree.$branch"
        r.keptBecause != null -> "Deleted. Its worktree was kept: ${r.keptBecause}.$branch"
        else -> "Deleted.$branch"
    }
}

/** Destructive choices from the bottom, as iOS's confirmation dialog shows them. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ActionSheet(message: String, actions: List<Pair<String, () -> Unit>>, dismiss: () -> Unit) {
    ModalBottomSheet(dismiss, containerColor = Color.Transparent, dragHandle = null, shape = RoundedCornerShape(0.dp)) {
        Column(Modifier.padding(horizontal = 8.dp).navigationBarsPadding(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Column(Modifier.fillMaxWidth().card(14.dp, palette.card)) {
                Text(
                    message, Modifier.fillMaxWidth().padding(16.dp),
                    style = Type.footnote, color = palette.secondary, textAlign = TextAlign.Center,
                )
                actions.forEach { (label, run) ->
                    HorizontalDivider(thickness = 0.5.dp, color = palette.separator)
                    Text(
                        label,
                        Modifier.fillMaxWidth().clickable {
                            dismiss()
                            run()
                        }.padding(vertical = 16.dp).testTag("action-$label"),
                        style = Type.body.copy(fontSize = 20.sp), color = palette.red, textAlign = TextAlign.Center,
                    )
                }
            }
            Text(
                "Cancel",
                Modifier.fillMaxWidth().card(14.dp, palette.card).clickable(onClick = dismiss).padding(vertical = 16.dp),
                style = Type.headline.copy(fontSize = 20.sp), color = palette.accent, textAlign = TextAlign.Center,
            )
            Spacer(Modifier.height(8.dp))
        }
    }
}

/** A disclosure group, as iOS shows "Details". */
@Composable
private fun Details(s: Session) {
    var open by remember { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(Modifier.fillMaxWidth().clickable { open = !open }.padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Details", Modifier.weight(1f), style = Type.subheadline, color = palette.label)
            Icon(
                Icons.AutoMirrored.Filled.KeyboardArrowRight, null,
                Modifier.size(18.dp).rotate(if (open) 90f else 0f), tint = palette.accent,
            )
        }
        if (open) {
            Labeled("Project", s.projectId)
            Labeled("Machine", "${s.machineId} (${s.machineStatus})")
            s.herdrSession?.let { h ->
                Labeled("Herdr session", h)
                Text("On ${s.machineId}: herdr session attach $h", style = Type.caption, fontFamily = FontFamily.Monospace, color = palette.secondary)
            }
        }
    }
}

@Composable
fun Labeled(label: String, value: String) {
    Row(Modifier.fillMaxWidth()) {
        Text(label, Modifier.weight(1f), style = Type.subheadline, color = palette.label)
        Text(value, style = Type.subheadline, color = palette.secondary)
    }
}
