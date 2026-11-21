package com.digitaltracer.switchboard

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Fingerprint
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Dialog
import com.digitaltracer.switchboard.api.models.DialogOption
import com.digitaltracer.switchboard.api.models.DialogStep
import com.digitaltracer.switchboard.core.HubError
import java.util.UUID
import kotlin.coroutines.cancellation.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** One choice an agent waits on: a hook question's option or a dialog's. */
data class OptionItem(
    val id: String,
    val label: String,
    val detail: String? = null,
    /** Shown for context only when false: free text or a chat. */
    val offered: Boolean = true,
)

/** Options as radio rows, shared by hook questions and dialogs. One not on offer is dashed and can't be picked. */
@Composable
fun OptionList(options: List<OptionItem>, selected: Set<String>, enabled: Boolean, tap: (OptionItem) -> Unit) {
    Column(Modifier.fillMaxWidth()) {
        options.forEachIndexed { i, o ->
            RadioRow(
                o.label, o.id in selected, Modifier.testTag("option-${o.label}"), offered = o.offered, first = i == 0,
                onClick = if (enabled) ({ tap(o) }) else null,
            ) {
                o.detail?.takeIf { it.isNotEmpty() && it != o.label }?.let { Note(it) }
                if (!o.offered) Note("Answered at its terminal, not from the phone")
            }
        }
    }
}

/** A group of questions as chips: the open one filled, the answered ones ticked. They show progress; they aren't buttons. */
@Composable
private fun StepTabs(steps: List<DialogStep>) {
    Row(Modifier.testTag("dialog-steps")) {
        ChipRow { steps.forEach { s -> Chip(if (s.answered) "✓ ${s.label}" else s.label, s.current) } }
    }
}

/** The dialog on an agent's screen, followed while the Agent page shows its card. */
@Stable
class DialogWatch(val machine: String, val agent: String) {
    var dialog by mutableStateOf<Dialog?>(null)
        private set
    /** Waiting on something the phone can't show as options. */
    var waitingElsewhere by mutableStateOf(false)
        private set

    suspend fun follow(model: AppModel) {
        while (model.foreground) {
            model.hub?.let { hub ->
                runCatching { hub.dialog(machine, agent) }.getOrNull()?.let {
                    dialog = it.dialog
                    waitingElsewhere = it.dialog == null && it.waiting == true
                }
            }
            delay(if (dialog != null || waitingElsewhere) 3_000 else 5_000)
        }
    }
}

/** On the Agent page: the agent is waiting, with a way to answer. */
@Composable
fun WaitingCard(watch: DialogWatch, answer: () -> Unit, screen: () -> Unit) {
    val d = watch.dialog
    if (d != null) {
        CardBox(Modifier.testTag("dialog-waiting")) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                StatusGlyph(Glyph.Need)
                val n = d.steps.orEmpty().count { !it.label.lowercase().startsWith("submit") }
                Text(if (n > 0) "It has $n questions" else "It's waiting for you", style = Type.title, color = palette.warn)
            }
            Text(d.prompt, style = Type.say, color = palette.ink)
            BigButton(if (d.answerable) "Answer" else "See the question", Modifier.fillMaxWidth().testTag("answerDialog"), onClick = answer)
        }
    } else if (watch.waitingElsewhere) {
        CardBox(Modifier.testTag("dialog-elsewhere")) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                StatusGlyph(Glyph.Need)
                Text("It's waiting for you", style = Type.title, color = palette.warn)
            }
            Note("On something the phone can't show as options. Look at its screen, or answer it at the machine.", color = palette.sub)
            BigButton("See its screen", Modifier.fillMaxWidth(), ButtonKind.Outline, onClick = screen)
        }
    }
}

/**
 * A decision as its own page: the question as the headline, its options as radio rows, one button
 * at the bottom. Any option but the deny asks for a fingerprint or the screen lock. In a group, each
 * pick is sent with Next, and only the one that submits them asks. A screen it doesn't recognise
 * is shown read-only.
 */
@Composable
fun DecisionScreen(model: AppModel, route: Route.Decision) {
    val scope = rememberCoroutineScope()
    var dialog by remember { mutableStateOf<Dialog?>(null) }
    var waitingElsewhere by remember { mutableStateOf(false) }
    var loaded by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var result by remember { mutableStateOf<String?>(null) }
    // The option picked on the phone, sent only with the button.
    var picked by remember { mutableStateOf<String?>(null) }
    // Cancel on a group's last step was tapped once; a second tap declines every question.
    var cancelArmed by remember { mutableStateOf(false) }
    suspend fun load() {
        val hub = model.hub ?: return
        if (busy) return
        // A failed read keeps what is shown.
        val d = runCatching { hub.dialog(route.machine, route.agent) }.getOrNull() ?: return
        if (d.dialog?.occurrenceId != dialog?.occurrenceId) {
            if (dialog != null && d.dialog != null) result = null
            picked = null
            cancelArmed = false
        }
        dialog = d.dialog
        waitingElsewhere = d.dialog == null && d.waiting == true
        loaded = true
    }
    LaunchedEffect(route, model.wakes, model.foreground) {
        while (model.foreground) {
            load()
            delay(3_000)
        }
    }
    // Puts up a dialog the hub sent back with an answer or a step back, with no second read.
    fun show(next: Dialog) {
        dialog = next
        waitingElsewhere = false
        picked = null
        cancelArmed = false
    }
    fun send(d: Dialog, option: DialogOption) {
        scope.launch {
            val hub = model.hub ?: return@launch
            busy = true
            var shown = false
            result = try {
                val next = hub.answer(route.machine, route.agent, d, option, "Answer ${route.label}: ${option.label}", UUID.randomUUID().toString()).next
                if (next != null) {
                    // A group's next question comes back with the answer.
                    show(next)
                    shown = true
                    null
                } else {
                    dialog = null
                    picked = null
                    "Answered: ${option.label}."
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: KeyError.Cancelled) {
                "Not answered: the fingerprint check was cancelled."
            } catch (e: HubError.Hub) {
                if (e.code == "dialog_changed") "The question changed; here it is again." else e.message
            } catch (e: Exception) {
                e.message
            }
            busy = false
            if (!shown) load()
        }
    }
    fun back(d: Dialog) {
        scope.launch {
            val hub = model.hub ?: return@launch
            busy = true
            var shown = false
            result = try {
                hub.back(route.machine, route.agent, d)?.let { show(it); shown = true }
                null
            } catch (e: CancellationException) {
                throw e
            } catch (e: HubError.Hub) {
                if (e.code == "dialog_changed") "The question changed; here it is again." else e.message
            } catch (e: Exception) {
                e.message
            }
            busy = false
            if (!shown) load()
        }
    }

    val d = dialog
    InboxPage(
        top = { PageTop("Back", model::pop, route.meta) },
        bottom = d?.let {
            {
                Column(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Note(if (it.answerable) hint(it) else why(it.reason))
                    result?.let { r -> Note(r, Modifier.testTag("dialog-result"), palette.sub) }
                    if (it.answerable) Buttons(it, busy, picked, cancelArmed, { cancelArmed = true }, ::send, ::back)
                }
            }
        },
    ) {
        if (d == null) {
            Column(Modifier.padding(horizontal = 20.dp).padding(top = 22.dp).testTag(if (waitingElsewhere) "dialog-elsewhere" else "dialog-none")) {
                when {
                    !loaded -> StateRow("Reading its screen…", busy = true)
                    waitingElsewhere -> {
                        StateLine(stateText("It's waiting on something else."), size = 30f)
                        Note("Switchboard can't show it as options here. Look at its screen, or answer it at the machine.", Modifier.padding(top = 12.dp), palette.sub)
                    }
                    else -> StateLine(stateText("Nothing to answer now."), size = 30f)
                }
                result?.let { Note(it, Modifier.padding(top = 12.dp).testTag("dialog-result"), palette.sub) }
            }
            return@InboxPage
        }
        val last = submitStep(d)
        val steps = d.steps.orEmpty()
        val permission = steps.isEmpty() && d.options.any { it.isDeny }
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).padding(bottom = 24.dp).testTag("dialog-card")) {
            Row(Modifier.padding(top = 6.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                StatusGlyph(Glyph.Need)
                Text(
                    if (permission) "${route.label} asks permission" else "${route.label} is waiting",
                    style = Type.text.copy(fontWeight = FontWeight.Medium), color = palette.warn, maxLines = 2,
                )
            }
            if (steps.isNotEmpty()) Row(Modifier.padding(top = 16.dp)) { StepTabs(steps) }
            val (question, detail) = split(d.prompt)
            StateLine(stateText(if (last != null) "Ready to submit your answers?" else question), Modifier.padding(top = 22.dp), 30f)
            if (last == null && detail != null) {
                if (permission) Column(Modifier.padding(top = 18.dp)) { CodeBlock(detail, wraps = true) }
                else Note(detail, Modifier.padding(top = 10.dp), palette.sub)
            }
            if (last != null) {
                if (d.prompt != "Ready to submit your answers?") Note(d.prompt, Modifier.padding(top = 10.dp), palette.sub)
                Column(Modifier.padding(top = 22.dp)) {
                    steps.filter { it.answered }.forEachIndexed { i, s -> InboxRow(s.label, "Answered", first = i == 0) }
                    Hairline()
                }
            } else {
                Column(Modifier.padding(top = 22.dp)) {
                    OptionList(
                        d.options.map { OptionItem(it.id, it.label, offered = it.offered && d.answerable) },
                        setOfNotNull(picked), !busy,
                    ) { o -> picked = o.id }
                    Hairline()
                }
            }
        }
    }
}

/** The bottom of a decision: Back when the group has an earlier question, then the one button. */
@Composable
private fun Buttons(
    d: Dialog, busy: Boolean, picked: String?, cancelArmed: Boolean, arm: () -> Unit,
    send: (Dialog, DialogOption) -> Unit, back: (Dialog) -> Unit,
) {
    val last = submitStep(d)
    val chosen = d.options.firstOrNull { it.id == picked }
    // One question of a group submits nothing, so it goes without a fingerprint.
    val next = chosen?.provisional == true
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        if (d.canGoBack == true) {
            BigButton("Back", Modifier.width(140.dp).testTag("dialog-back"), ButtonKind.Outline, enabled = !busy) { back(d) }
        }
        if (last != null) {
            BigButton(last.first.label, Modifier.weight(1f).testTag("dialog-confirm"), enabled = !busy, icon = Icons.Filled.Fingerprint) {
                send(d, last.first)
            }
        } else {
            BigButton(
                when {
                    next -> "Next question"
                    chosen?.isDeny == true -> "Confirm no"
                    else -> "Confirm"
                },
                Modifier.weight(1f).testTag("dialog-confirm"),
                enabled = chosen != null && !busy,
                icon = if (!next && chosen?.isDeny != true) Icons.Filled.Fingerprint else null,
            ) { chosen?.let { send(d, it) } }
        }
    }
    last?.second?.let { cancel ->
        BigButton(
            if (cancelArmed) "Tap again to decline every question" else "Cancel and decline every question",
            Modifier.fillMaxWidth().testTag("dialog-cancel"), ButtonKind.Plain, enabled = !busy,
            color = if (cancelArmed) palette.bad else palette.sub,
        ) { if (cancelArmed) send(d, cancel) else arm() }
    }
}

/** The question, and what it's about: `Would you like to run the following command?` and `cargo test --workspace`. */
private fun split(prompt: String): Pair<String, String?> {
    val lines = prompt.split("\n").map { it.trim() }.filter { it.isNotEmpty() }
    if (lines.size < 2) return prompt to null
    return lines.first() to lines.drop(1).joinToString("\n") { it.removePrefix("$ ") }
}

/** A group's last step when all it offers is to submit or cancel: buttons, not a list. */
private fun submitStep(d: Dialog): Pair<DialogOption, DialogOption?>? {
    if (!d.answerable || d.steps.isNullOrEmpty()) return null
    val offered = d.options.filter { it.offered }
    val submit = offered.singleOrNull { !it.isDeny }?.takeIf { it.provisional != true && offered.size <= 2 } ?: return null
    return submit to offered.firstOrNull { it.isDeny }
}

private fun hint(d: Dialog): String = when {
    !d.steps.isNullOrEmpty() -> "Pick an option for each question. Your fingerprint is asked once, when you submit."
    d.options.any { it.isDeny } -> "Saying no needs no fingerprint. Any other answer asks for it."
    else -> "Pick an option, then confirm. Your fingerprint is asked when you do."
}

private fun why(reason: String?): String = when (reason) {
    "hooked" -> "Answer it as the approval on its session."
    "answered" -> "Already answered from the phone."
    "read_only" -> "This kind of question can't be answered from the phone. Answer it at its terminal."
    "hook_pending", "hook_failed" -> "Its approval is being handled; answer it at its terminal if it stays."
    else -> "Answer it at its terminal, on its machine."
}
