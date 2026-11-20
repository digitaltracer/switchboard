package com.digitaltracer.switchboard

import android.text.format.DateUtils
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Fingerprint
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Approval
import com.digitaltracer.switchboard.api.models.Changes
import com.digitaltracer.switchboard.api.models.DecisionResponse
import com.digitaltracer.switchboard.api.models.Session
import com.digitaltracer.switchboard.core.Countdown
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

val palette: Palette @Composable get() = LocalPalette.current

/** How a session's or agent's state reads at a glance: its word and its glyph. */
data class SessionStatus(val text: String, val glyph: Glyph) {
    val color: Color
        @Composable get() = when (glyph) {
            Glyph.Need -> palette.warn
            Glyph.Work -> palette.ac
            Glyph.Done -> palette.ok
            Glyph.Idle, Glyph.End -> palette.sub
        }

    companion object {
        fun of(s: Session): SessionStatus = when {
            s.hookFailed -> SessionStatus("Stuck", Glyph.Need)
            s.pendingApprovals.isNotEmpty() -> SessionStatus("Needs you", Glyph.Need)
            s.lifecycle == "starting" -> SessionStatus("Starting", Glyph.Work)
            s.lifecycle != "running" -> SessionStatus(s.lifecycle.capitalized(), Glyph.End)
            s.machineStatus != "ready" -> SessionStatus("${s.machineId} ${s.machineStatus}", Glyph.Idle)
            else -> agent(s.agentStatus)
        }

        fun agent(status: String): SessionStatus = when (status) {
            "working" -> SessionStatus("Working", Glyph.Work)
            "blocked" -> SessionStatus("Needs you", Glyph.Need)
            "idle" -> SessionStatus("Idle", Glyph.Idle)
            "done" -> SessionStatus("Done", Glyph.Done)
            "ended" -> ended
            else -> SessionStatus(status.capitalized(), Glyph.Idle)
        }

        val ended = SessionStatus("Ended", Glyph.End)

        /** A message just went, and the agent hasn't been seen to pick it up yet. */
        val sent = SessionStatus("Sent", Glyph.Work)
    }
}

fun String.capitalized() = replaceFirstChar { it.uppercase() }

val Session.live get() = lifecycle == "running" || lifecycle == "starting"

/** "Claude Code" or "Codex". */
fun agentName(kind: String?): String? = when (kind) {
    null -> null
    "claude", "claude-code" -> "Claude Code"
    "codex" -> "Codex"
    else -> kind.capitalized()
}

@Composable
fun Dot(color: Color, size: Dp = 8.dp) {
    Box(Modifier.size(size).background(color, CircleShape))
}

/** A rounded fill. */
fun Modifier.card(radius: Dp = 20.dp, color: Color) = clip(RoundedCornerShape(radius)).background(color)

object Ago {
    /** `now`, `5m`, `2h` or `3d` since a hub timestamp in milliseconds. */
    fun short(ms: Long, now: Long = System.currentTimeMillis()): String {
        val s = maxOf(0, (now - ms) / 1000)
        return when {
            s < 60 -> "now"
            s < 3600 -> "${s / 60}m"
            s < 86400 -> "${s / 3600}h"
            else -> "${s / 86400}d"
        }
    }

    /** "just now", "12m ago", "6 days ago". */
    fun long(ms: Long, now: Long = System.currentTimeMillis()): String {
        val s = maxOf(0, (now - ms) / 1000)
        return when {
            s < 60 -> "just now"
            s < 3600 -> "${s / 60}m ago"
            s < 86400 -> "${s / 3600}h ago"
            s < 172_800 -> "yesterday"
            else -> "${s / 86400} days ago"
        }
    }
}

/**
 * A pending approval as a card, with its countdown. A permission or plan is approved with a
 * fingerprint or the screen lock, or denied; a question is answered by picking its options, then
 * sending them the same way. Unanswered at zero, the hub denies it.
 */
@Composable
fun ApprovalCard(model: AppModel, approval: Approval, session: Session, open: (() -> Unit)? = null, decided: suspend () -> Unit = {}) {
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    var result by remember { mutableStateOf<String?>(null) }
    var picked by remember { mutableStateOf<Map<Int, Set<String>>>(emptyMap()) }
    var now by remember { mutableLongStateOf(Countdown.nowMs()) }
    LaunchedEffect(approval.id) {
        while (true) {
            now = Countdown.nowMs()
            delay(1_000)
        }
    }
    val title = when (approval.kind) {
        "question" -> if (open == null) "It has a question" else "${session.label} has a question"
        "plan" -> if (open == null) "Its plan, for you to approve" else "${session.label} has a plan for you"
        else -> if (open == null) "Run this?" else "${session.label} wants to run"
    }

    fun decide(call: suspend () -> DecisionResponse) {
        scope.launch {
            busy = true
            result = try {
                describe(call())
            } catch (e: CancellationException) {
                throw e
            } catch (e: KeyError.Cancelled) {
                null
            } catch (e: Exception) {
                e.message
            }
            busy = false
            decided()
        }
    }

    CardBox(Modifier.testTag("approval-${approval.id}")) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            StatusGlyph(Glyph.Need)
            Text(
                title,
                Modifier.weight(1f).then(if (open != null) Modifier.clickable(onClick = open) else Modifier),
                style = Type.title, color = palette.ink, maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
        }
        when (approval.kind) {
            "question" -> approval.questionOptions.forEachIndexed { i, q ->
                Column {
                    Text(q.question, style = Type.say, color = palette.ink)
                    if (q.multiSelect) Note("Pick any that apply.", Modifier.padding(top = 4.dp))
                    OptionList(q.options.map { OptionItem(it.id, it.label, it.description) }, picked[i].orEmpty(), !busy) { o ->
                        val now = picked[i].orEmpty()
                        picked = picked + (i to if (q.multiSelect) (if (o.id in now) now - o.id else now + o.id) else setOf(o.id))
                    }
                }
            }
            "plan" -> Column(Modifier.heightIn(max = 260.dp).verticalScroll(rememberScrollState())) { MarkdownText(approval.plan.orEmpty()) }
            else -> CodeBlock(approval.inputText, wraps = approval.inputText.length > 60)
        }
        val total = (approval.deadline - approval.createdAt).coerceAtLeast(1)
        CountdownBar((approval.deadline - now).toFloat() / total)
        Text("Denies itself in ${Countdown.text(approval.deadline, now)}", style = Type.mono(13f, FontWeight.Medium), color = palette.warn)
        result?.let { Note(it, color = palette.sub) }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            BigButton("Deny", Modifier.weight(1f).testTag("deny-${approval.id}"), ButtonKind.Outline, enabled = !busy) {
                model.hub?.let { hub -> decide { hub.deny(approval.id, UUID.randomUUID().toString()) } }
            }
            val answered = approval.questionOptions.indices.all { picked[it].orEmpty().isNotEmpty() }
            if (approval.kind != "question" || answered) {
                val text = if (approval.kind == "question") "Send" else "Approve"
                BigButton(text, Modifier.weight(1.5f).testTag("approve-${approval.id}"), enabled = !busy, icon = Icons.Filled.Fingerprint) {
                    model.hub?.let { hub ->
                        val reason = "Approve ${approval.toolName} in ${session.label}"
                        val id = UUID.randomUUID().toString()
                        decide {
                            if (approval.kind == "permission") {
                                hub.approve(approval.id, reason, id)
                            } else {
                                hub.answer(approval, picked.keys.sorted().flatMap { picked[it].orEmpty().sorted() }, reason, id)
                            }
                        }
                    }
                }
            }
        }
    }
}

private fun describe(r: DecisionResponse): String = when {
    r.replayed -> "Already decided: ${r.outcome}."
    r.outcome == "allow" -> if (r.delivery == "unknown") "Approved; the hub lost the connection, so it may have run." else "Approved."
    else -> "Denied."
}

/** What this phone last said there, kept on the phone. */
@Composable
fun SaidBubble(said: AppModel.Said) {
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(4.dp)) {
        MeBubble(said.text)
        Text("You said · ${DateUtils.getRelativeTimeSpanString(said.at)}", style = Type.meta, color = palette.mut)
    }
}

/** The owner's own words, in a dark bubble on the right. */
@Composable
fun MeBubble(text: String) {
    Box(Modifier.fillMaxWidth().padding(start = 48.dp), Alignment.CenterEnd) {
        Text(
            text,
            Modifier.clip(RoundedCornerShape(18.dp, 18.dp, 4.dp, 18.dp)).background(palette.ink).padding(horizontal = 14.dp, vertical = 10.dp),
            style = Type.message, color = palette.inv,
        )
    }
}

/** "3 changed files". */
fun Changes.title(): String {
    val n = files.size
    return if (n == 1) "1 changed file" else "$n${if (truncated) "+" else ""} changed files"
}

/** Changed files as rows: the path, then what changed. */
@Composable
fun ChangesList(changes: Changes) {
    Column(Modifier.fillMaxWidth()) {
        changes.files.forEachIndexed { i, f ->
            if (i > 0) Hairline()
            Row(Modifier.fillMaxWidth().heightIn(min = 44.dp).padding(vertical = 8.dp).testTag(f.path), verticalAlignment = Alignment.CenterVertically) {
                Text(f.path, Modifier.weight(1f), style = Type.mono(12.5f), color = palette.ink, maxLines = 1, overflow = TextOverflow.StartEllipsis)
                if (f.added == null && f.deleted == null) {
                    Text("new", style = Type.mono(12.5f), color = palette.ok)
                } else {
                    Text("+${f.added ?: 0} ", style = Type.mono(12.5f), color = palette.ok)
                    Text("−${f.deleted ?: 0}", style = Type.mono(12.5f), color = palette.bad)
                }
            }
        }
    }
}
