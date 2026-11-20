package com.digitaltracer.switchboard

import android.view.HapticFeedbackConstants
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
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.outlined.Mic
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
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
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.MessagePreview
import com.digitaltracer.switchboard.api.models.Skill
import com.digitaltracer.switchboard.api.models.TargetView
import com.digitaltracer.switchboard.core.HubError
import com.digitaltracer.switchboard.core.RouteOption
import com.digitaltracer.switchboard.core.RouteOptions
import com.digitaltracer.switchboard.core.SkillSearch
import com.digitaltracer.switchboard.core.StartFlow
import com.digitaltracer.switchboard.core.StartState
import com.digitaltracer.switchboard.core.sessionTarget
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Review before sending: the words, heard or typed, and where they'll go. The hub's preview
 * picks the destination; the user can pick another.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SendSheet(model: AppModel, voice: Voice, listen: Listen) {
    var preview by remember { mutableStateOf<MessagePreview?>(null) }
    var routes by remember { mutableStateOf(RouteOptions.Result(emptyList(), null)) }
    var picked by remember { mutableStateOf<String?>(null) }
    // A project picked under "New task in…", offered alongside the hub's options.
    var newProject by remember { mutableStateOf<String?>(null) }
    var note by remember { mutableStateOf<String?>(null) }
    var sending by remember { mutableStateOf(false) }
    // One per draft and destination, so a retried send is never typed twice.
    var requestId by remember { mutableStateOf(UUID.randomUUID().toString()) }
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    val focus = remember { FocusRequester() }
    val current = voice.target ?: model.current
    val shown = newProject?.let { routes.withNewTask(it) } ?: routes
    val chosen = shown.options.firstOrNull { it.id == (picked ?: shown.suggested) }
    val unoffered = shown.projectsWithoutNewTask(model.projects.map { it.id })
    val empty = voice.text.isBlank()

    // Asks the hub where the text would go, after a short pause in typing.
    LaunchedEffect(voice.text) {
        requestId = UUID.randomUUID().toString()
        val text = voice.text.trim()
        val hub = model.hub
        if (text.isEmpty() || hub == null) {
            preview = null
            routes = RouteOptions.Result(emptyList(), null)
            return@LaunchedEffect
        }
        delay(300)
        try {
            val p = hub.preview(text, current)
            preview = p
            routes = RouteOptions.build(p, model.sessions, voice.target)
            val offered = newProject?.let { routes.withNewTask(it) } ?: routes
            if (picked != null && offered.options.none { it.id == picked }) picked = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            note = e.message
        }
    }
    LaunchedEffect(Unit) { if (voice.text.isEmpty()) focus.requestFocus() }

    // A lost answer: the hub may have acted, so look the request up before calling it failed.
    suspend fun reconcile(text: String) {
        val hub = model.hub ?: return
        note = "Not sure that got through. Checking…"
        var state: StartState = StartState.Unknown(requestId)
        for (attempt in 0 until 30) {
            state = StartFlow.reconcile(hub, requestId, attempt)
            if (state !is StartState.Unknown) break
            delay(2_000)
        }
        when (val s = state) {
            is StartState.Started -> {
                runCatching { hub.session(s.sessionId) }.onSuccess {
                    model.record(text, sessionTarget(it), started = true)
                }
                voice.finish()
            }
            is StartState.Unknown -> note = "Still not sure it went through. Sending it again is safe."
            is StartState.Failed -> note = "That didn't reach the hub. Send it again."
        }
    }

    fun send() {
        val hub = model.hub ?: return
        val option = chosen ?: return
        val text = voice.text.trim()
        scope.launch {
            sending = true
            try {
                val r = when (val a = option.action) {
                    RouteOption.Action.AsPreviewed -> hub.message(text, requestId, current)
                    is RouteOption.Action.Start -> hub.message(text, requestId, current, projectId = a.projectId)
                    is RouteOption.Action.Send -> hub.message(text, requestId, current, target = a.target)
                }
                note = r.message
                when (r.outcome) {
                    "started", "sent", "selected" -> {
                        // Only `use <x>` makes a default; a send doesn't, so the home screen asks again.
                        if (r.outcome == "selected") model.current = r.current
                        model.record(text, r.current, started = r.outcome == "started")
                        view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
                        voice.finish()
                    }
                    "choose_project", "choose_target" -> {
                        // The hub still couldn't tell; its options are the destinations now.
                        val asked = MessagePreview(r.outcome, r.message, r.options)
                        preview = asked
                        routes = RouteOptions.build(asked, model.sessions, voice.target)
                        picked = null
                    }
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: HubError) {
                if (e.outcomeUnknown) reconcile(text) else note = e.message
            } catch (e: Exception) {
                note = e.message
            } finally {
                sending = false
            }
        }
    }

    ModalBottomSheet(
        onDismissRequest = { voice.finish() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = palette.sf,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
    ) {
        Column(Modifier.fillMaxHeight(0.95f).imePadding().navigationBarsPadding()) {
            Row(Modifier.fillMaxWidth().height(44.dp).padding(horizontal = 20.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Cancel", Modifier.clickable { voice.finish() }.padding(vertical = 10.dp), style = Type.field.copy(fontWeight = FontWeight.Medium), color = palette.ac)
                Spacer(Modifier.weight(1f))
                Text(current?.let { "To ${it.label}" } ?: "Review before sending", style = Type.meta, color = palette.mut, maxLines = 1)
            }
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).padding(top = 8.dp)) {
                voice.error?.let { Banner(it, Modifier.padding(bottom = 12.dp)) }
                voice.notice?.let { Note(it, Modifier.padding(bottom = 12.dp)) }
                if (voice.retrying) {
                    Row(Modifier.padding(bottom = 12.dp).testTag("voiceRetrying"), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                        CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = palette.mut)
                        Note("Transcribing with OpenAI…")
                    }
                } else if (voice.canRetry) {
                    BigButton("Retry", Modifier.fillMaxWidth().padding(bottom = 12.dp).testTag("voiceRetry"), ButtonKind.Outline) { voice.retry() }
                }
                val skillsShown = SkillSearch.isTyping(voice.text)
                val fieldStyle = (if (skillsShown) Type.mono(22f, FontWeight.Medium) else Type.draft).copy(color = palette.ink)
                BasicTextField(
                    voice.text, { voice.text = it },
                    Modifier.fillMaxWidth().heightIn(min = 40.dp).focusRequester(focus).testTag("composeField"),
                    textStyle = fieldStyle,
                    cursorBrush = SolidColor(palette.ac),
                    maxLines = 8,
                    decorationBox = { inner ->
                        if (voice.text.isEmpty()) {
                            Text(current?.let { "Message ${it.label}" } ?: "Say or type what you want done", style = fieldStyle.copy(color = palette.mut))
                        }
                        inner()
                    },
                )
                if (skillsShown && current != null) {
                    SkillList(model, current, voice.text) { voice.text = "$it " }
                }
                if (shown.options.isNotEmpty()) {
                    SectionHead(if (shown.options.size == 1) "Goes to" else "Where should this go?", top = 30.dp)
                    preview?.message?.let { Note(it, Modifier.padding(bottom = 8.dp).testTag("previewMessage")) }
                    shown.options.forEachIndexed { i, o ->
                        RouteRow(o, o.id == chosen?.id, i == 0) {
                            picked = o.id
                            requestId = UUID.randomUUID().toString()
                        }
                    }
                    if (unoffered.isNotEmpty()) {
                        NewTaskRow(unoffered) { p ->
                            newProject = p
                            picked = RouteOptions.newTask(p).id
                            requestId = UUID.randomUUID().toString()
                        }
                    }
                    Hairline()
                } else if (preview?.outcome == "refused") {
                    Note(preview?.message.orEmpty(), Modifier.padding(top = 14.dp))
                }
                note?.let { Note(it, Modifier.padding(top = 12.dp).testTag("composeNote"), palette.sub) }
            }
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                RoundIcon(Icons.Outlined.Mic, "Keep talking") { listen(voice.target, keepText = true) }
                BigButton(
                    chosen?.verb ?: if (empty) "Say or type something" else "Pick where this goes",
                    Modifier.weight(1f).testTag("composeSend"),
                    enabled = chosen != null && !empty && !voice.retrying, busy = sending,
                    shape = RoundedCornerShape(26.dp), icon = Icons.Filled.ArrowUpward,
                ) { send() }
            }
        }
    }
}

/** The last row under "Goes to": a menu of projects to start a new task in. */
@Composable
private fun NewTaskRow(projects: List<String>, pick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        RadioRow("New task in…", false, Modifier.testTag("routeNewTask"), onClick = { open = true }) { Note("Pick a project") }
        DropdownMenu(open, { open = false }) {
            projects.forEach { p ->
                DropdownMenuItem({ Text(p, style = Type.field) }, {
                    open = false
                    pick(p)
                })
            }
        }
    }
}

@Composable
private fun RouteRow(option: RouteOption, selected: Boolean, first: Boolean, pick: () -> Unit) {
    RadioRow(option.title, selected, Modifier.testTag("route-${option.id}"), first = first, onClick = pick) {
        Text(option.detail, style = Type.sub, color = palette.sub, maxLines = 1)
    }
}

/** The agent's skills matching what's typed after `/`; picking one puts it in the message. */
@Composable
private fun SkillList(model: AppModel, target: TargetView, typed: String, pick: (String) -> Unit) {
    val key = AppModel.saidKey(target)
    var skills by remember(key) { mutableStateOf<List<Skill>?>(null) }
    var error by remember(key) { mutableStateOf<String?>(null) }
    LaunchedEffect(key) {
        val hub = model.hub ?: return@LaunchedEffect
        try {
            val id = target.sessionId
            val agent = target.agent
            skills = when {
                id != null -> hub.skills(id)
                agent != null -> hub.agentSkills(target.machineId, agent)
                else -> emptyList()
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message
        }
    }
    Column(Modifier.fillMaxWidth()) {
        val all = skills
        when {
            all != null -> {
                val shown = SkillSearch.filter(all, typed).take(30)
                SectionHead("Skills matching ${typed.trim()}", "${shown.size}", top = 30.dp)
                if (shown.isEmpty()) StateRow("No skill matches $typed.")
                shown.forEachIndexed { i, skill ->
                    InboxRow(
                        skill.name, skill.description.ifEmpty { null }, Modifier.testTag("skill-${skill.name}"), first = i == 0,
                        onClick = { pick(skill.name) },
                        trailing = { sourceLabel(skill.source)?.let { Text(it, style = Type.meta, color = palette.mut) } },
                    )
                }
                Hairline()
                Note("Tapping a skill puts its name in the message. Nothing is sent until you tap Send.", Modifier.padding(top = 12.dp))
            }
            error != null -> StateRow("Couldn't list its skills: $error", error = true)
            else -> StateRow("Listing its skills…", busy = true)
        }
    }
}

private fun sourceLabel(source: String): String? = when (source) {
    "repo" -> "this repo"
    "plugin" -> "plugin"
    "builtin" -> "built in"
    "user" -> "yours"
    else -> null
}
