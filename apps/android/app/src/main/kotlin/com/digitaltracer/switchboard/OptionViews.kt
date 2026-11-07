package com.digitaltracer.switchboard

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.HelpOutline
import androidx.compose.material.icons.filled.CheckCircle
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Dialog
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
    /** Shown for context only when false: free text, a chat, or a standing permission. */
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
    suspend fun load() {
        val hub = model.hub ?: return
        if (busy) return
        // A failed read keeps what is shown.
        val d = runCatching { hub.dialog(machine, agent) }.getOrNull() ?: return
        if (dialog != null && d.dialog != null && d.dialog?.occurrenceId != dialog?.occurrenceId) result = null
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
        OptionList(
            d.prompt,
            d.options.map { OptionItem(it.id, it.label, offered = it.offered && d.answerable) },
            setOfNotNull(d.selected),
            !busy,
        ) { o ->
            val option = d.options.first { it.id == o.id }
            scope.launch {
                val hub = model.hub ?: return@launch
                busy = true
                result = try {
                    hub.answer(machine, agent, d, option, "Answer $label: ${option.label}", UUID.randomUUID().toString())
                    dialog = null
                    "Answered: ${option.label}."
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
                load()
            }
        }
        Text(
            if (d.answerable) "Tap an option to answer. The tick is what its screen has selected now." else why(d.reason),
            style = Type.footnote, color = palette.secondary,
        )
        result?.let { Text(it, style = Type.footnote, color = palette.secondary) }
    }
}

private fun why(reason: String?): String = when (reason) {
    "hooked" -> "Answer it as the approval above."
    "answered" -> "Already answered from the phone."
    "read_only" -> "This kind of question can't be answered from the phone. Answer it in Herdr."
    "hook_pending", "hook_failed" -> "Its approval is being handled; answer it in Herdr if it stays."
    else -> "Answer it in Herdr, on its machine."
}
