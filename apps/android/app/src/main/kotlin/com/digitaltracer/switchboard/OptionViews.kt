package com.digitaltracer.switchboard

import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.ArrowForward
import androidx.compose.material.icons.automirrored.outlined.HelpOutline
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Fingerprint
import androidx.compose.material.icons.outlined.Circle
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextAlign
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

/** The option card's list, shared by hook questions and dialogs on an agent's screen. */
@Composable
fun OptionList(prompt: String, options: List<OptionItem>, selected: Set<String>, enabled: Boolean, tap: (OptionItem) -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(prompt, style = Type.body, color = palette.label)
        options.forEach { o ->
            Row(
                Modifier.fillMaxWidth().card(12.dp, palette.fill)
                    .clickable(enabled = enabled && o.offered) { tap(o) }
                    .alpha(if (o.offered) 1f else 0.45f)
                    .padding(horizontal = 12.dp, vertical = 10.dp)
                    .testTag("option-${o.label}"),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(
                    if (o.id in selected) Icons.Filled.CheckCircle else Icons.Outlined.Circle, null, Modifier.size(20.dp),
                    tint = if (o.offered) palette.accent else palette.secondary,
                )
                Column(Modifier.weight(1f)) {
                    Text(o.label, style = Type.callout, color = palette.label)
                    o.detail?.takeIf { it.isNotEmpty() && it != o.label }?.let { Text(it, style = Type.footnote, color = palette.secondary) }
                }
            }
        }
    }
}

/**
 * The dialog on an agent's screen, answered from the phone when Switchboard recognises it. Any
 * option but the deny asks for a fingerprint or the screen lock. It reads the screen itself while
 * shown, so a dialog that appears after the page opened still shows, and says so when the agent
 * waits on something else.
 */
@Composable
fun DialogCard(model: AppModel, machine: String, agent: String, label: String) {
    val scope = rememberCoroutineScope()
    var dialog by remember { mutableStateOf<Dialog?>(null) }
    // Waiting on something the phone can't show as options.
    var waitingElsewhere by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var result by remember { mutableStateOf<String?>(null) }
    // The option picked on the phone, sent only on Confirm.
    var picked by remember { mutableStateOf<String?>(null) }
    // Cancel on a group's last step was tapped once; a second tap declines every question.
    var cancelArmed by remember { mutableStateOf(false) }
    suspend fun load() {
        val hub = model.hub ?: return
        if (busy) return
        // A failed read keeps what is shown.
        val d = runCatching { hub.dialog(machine, agent) }.getOrNull() ?: return
        if (d.dialog?.occurrenceId != dialog?.occurrenceId) {
            if (dialog != null && d.dialog != null) result = null
            picked = null
            cancelArmed = false
        }
        dialog = d.dialog
        waitingElsewhere = d.dialog == null && d.waiting == true
    }
    LaunchedEffect(machine, agent, model.wakes, model.foreground) {
        while (model.foreground) {
            load()
            delay(if (dialog != null || waitingElsewhere) 3_000 else 5_000)
        }
    }
    val d = dialog
    if (d == null) {
        if (!waitingElsewhere && result == null) return
        Column(
            Modifier.fillMaxWidth().card(20.dp, palette.card).padding(16.dp).testTag("dialog-elsewhere"),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (waitingElsewhere) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Icon(Icons.AutoMirrored.Outlined.HelpOutline, null, Modifier.size(20.dp), tint = palette.needs)
                    Text("$label is waiting", style = Type.headline, color = palette.needs)
                }
                Text(
                    "On something Switchboard can't show as options here. Check its screen, or answer it in Herdr.",
                    style = Type.footnote, color = palette.secondary,
                )
            }
            result?.let { Text(it, style = Type.footnote, color = palette.secondary) }
        }
        return
    }
    Column(
        Modifier.fillMaxWidth().card(20.dp, palette.card).padding(16.dp).testTag("dialog-card"),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.AutoMirrored.Outlined.HelpOutline, null, Modifier.size(20.dp), tint = palette.needs)
            Text("$label is waiting", style = Type.headline, color = palette.needs)
        }
        val steps = d.steps.orEmpty()
        if (steps.isNotEmpty()) StepTabs(steps)
        // Puts up a dialog the hub sent back with an answer or a step back, with no second read.
        fun show(next: Dialog) {
            dialog = next
            waitingElsewhere = false
            picked = null
            cancelArmed = false
        }
        fun send(option: DialogOption) {
            scope.launch {
                val hub = model.hub ?: return@launch
                busy = true
                var shown = false
                result = try {
                    val next = hub.answer(machine, agent, d, option, "Answer $label: ${option.label}", UUID.randomUUID().toString()).next
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
        fun back() {
            scope.launch {
                val hub = model.hub ?: return@launch
                busy = true
                var shown = false
                result = try {
                    hub.back(machine, agent, d)?.let { show(it); shown = true }
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
        // A step's main button, with Back beside it when the group has an earlier question.
        @Composable
        fun Actions(main: @Composable (Modifier) -> Unit) {
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                if (d.canGoBack == true) {
                    BigButton(
                        "Back", Icons.AutoMirrored.Filled.ArrowBack, palette.fill, palette.label, !busy,
                        Modifier.testTag("dialog-back").width(110.dp),
                    ) { back() }
                }
                main(Modifier.weight(1f))
            }
        }
        // A group's last step, when all it offers is to submit or cancel: buttons, not a list.
        val offered = d.options.filter { it.offered }
        val submit = offered.singleOrNull { !it.isDeny }
            ?.takeIf { d.answerable && steps.isNotEmpty() && it.provisional != true && offered.size <= 2 }
        if (submit != null) {
            Text(d.prompt, style = Type.body, color = palette.label)
            result?.let { Text(it, style = Type.footnote, color = palette.secondary) }
            Actions { m ->
                BigButton(
                    submit.label, Icons.Filled.Fingerprint, palette.accent, Color.White, !busy,
                    m.testTag("dialog-confirm"),
                ) { send(submit) }
            }
            offered.firstOrNull { it.isDeny }?.let { cancel ->
                Text(
                    if (cancelArmed) "Tap again to decline every question" else cancel.label,
                    Modifier.fillMaxWidth().clickable(enabled = !busy) { if (cancelArmed) send(cancel) else cancelArmed = true }
                        .padding(vertical = 6.dp).testTag("dialog-cancel"),
                    style = Type.subheadline, color = if (cancelArmed) palette.red else palette.secondary, textAlign = TextAlign.Center,
                )
            }
            return@Column
        }
        OptionList(
            d.prompt,
            d.options.map { OptionItem(it.id, it.label, offered = it.offered && d.answerable) },
            setOfNotNull(picked),
            !busy,
        ) { o -> picked = o.id }
        Text(
            when {
                !d.answerable -> why(d.reason)
                steps.isNotEmpty() -> "Pick an option for each question. Your fingerprint is asked once, when you submit."
                else -> "Pick an option, then confirm."
            },
            style = Type.footnote, color = palette.secondary,
        )
        result?.let { Text(it, style = Type.footnote, color = palette.secondary) }
        if (!d.answerable) return@Column
        val chosen = d.options.firstOrNull { it.id == picked }
        // One question of a group submits nothing, so it goes without a fingerprint.
        val next = chosen?.provisional == true
        Actions { m ->
        BigButton(
            if (next) "Next" else "Confirm",
            when {
                next -> Icons.AutoMirrored.Filled.ArrowForward
                chosen?.isDeny == true -> Icons.Filled.Check
                else -> Icons.Filled.Fingerprint
            },
            if (chosen == null) palette.fill else palette.accent, if (chosen == null) palette.secondary else Color.White,
            chosen != null && !busy, m.testTag("dialog-confirm"),
        ) { chosen?.let(::send) }
        }
    }
}

/** A group of questions as tabs: the open one filled, the answered ones ticked. */
@Composable
private fun StepTabs(steps: List<DialogStep>) {
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).testTag("dialog-steps"),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        steps.forEach { s ->
            val tint = if (s.current) Color.White else if (s.answered) palette.label else palette.secondary
            Row(
                Modifier.card(50.dp, if (s.current) palette.accent else palette.fill).padding(horizontal = 10.dp, vertical = 6.dp),
                horizontalArrangement = Arrangement.spacedBy(4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (s.answered) Icon(Icons.Filled.Check, null, Modifier.size(12.dp), tint = tint)
                Text(s.label, style = Type.footnote, color = tint)
            }
        }
    }
}

private fun why(reason: String?): String = when (reason) {
    "hooked" -> "Answer it as the approval above."
    "answered" -> "Already answered from the phone."
    "read_only" -> "This kind of question can't be answered from the phone. Answer it in Herdr."
    "hook_pending", "hook_failed" -> "Its approval is being handled; answer it in Herdr if it stays."
    else -> "Answer it in Herdr, on its machine."
}
