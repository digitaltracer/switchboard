package com.digitaltracer.switchboard

import android.text.format.DateUtils
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.automirrored.outlined.HelpOutline
import androidx.compose.material.icons.filled.Fingerprint
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.HourglassEmpty
import androidx.compose.material.icons.outlined.MonitorHeart
import androidx.compose.material.icons.outlined.PauseCircle
import androidx.compose.material.icons.outlined.StopCircle
import androidx.compose.material.icons.outlined.VerifiedUser
import androidx.compose.material.icons.outlined.Warning
import androidx.compose.material.icons.outlined.WifiOff
import androidx.compose.material.icons.outlined.RadioButtonUnchecked
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
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
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
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

val codeBackground = Color(0xFF1C1C1F)
private val codeText = Color(0xFFE6E6E6)

/** How a session's state reads at a glance. */
data class SessionStatus(val text: String, val kind: Kind, val symbol: ImageVector) {
    enum class Kind { Needs, Working, Done, Failed, Quiet }

    val color: Color
        @Composable get() = when (kind) {
            Kind.Needs -> palette.needs
            Kind.Working -> palette.working
            Kind.Done -> palette.done
            Kind.Failed -> palette.red
            Kind.Quiet -> palette.secondary
        }

    companion object {
        fun of(s: Session): SessionStatus = when {
            s.hookFailed -> SessionStatus("Stuck", Kind.Failed, Icons.Outlined.Warning)
            s.pendingApprovals.isNotEmpty() -> SessionStatus("Needs you", Kind.Needs, Icons.Outlined.VerifiedUser)
            s.lifecycle == "starting" -> SessionStatus("Starting", Kind.Quiet, Icons.Outlined.HourglassEmpty)
            s.lifecycle != "running" ->
                SessionStatus(s.lifecycle.capitalized(), if (s.lifecycle == "failed") Kind.Failed else Kind.Quiet, Icons.Outlined.StopCircle)
            s.machineStatus != "ready" -> SessionStatus("${s.machineId} ${s.machineStatus}", Kind.Quiet, Icons.Outlined.WifiOff)
            else -> agent(s.agentStatus)
        }

        fun agent(status: String): SessionStatus = when (status) {
            "working" -> SessionStatus("Working", Kind.Working, Icons.Outlined.MonitorHeart)
            "blocked" -> SessionStatus("Blocked", Kind.Needs, Icons.Outlined.PauseCircle)
            "idle" -> SessionStatus("Idle", Kind.Done, Icons.Outlined.CheckCircle)
            "done" -> SessionStatus("Done", Kind.Done, Icons.Outlined.CheckCircle)
            else -> SessionStatus(status.capitalized(), Kind.Quiet, Icons.Outlined.RadioButtonUnchecked)
        }
    }
}

fun String.capitalized() = replaceFirstChar { it.uppercase() }

val Session.live get() = lifecycle == "running" || lifecycle == "starting"

@Composable
fun StatusPill(status: SessionStatus) {
    Box(Modifier.height(24.dp).background(status.color.copy(alpha = 0.14f), CircleShape).padding(horizontal = 9.dp), Alignment.Center) {
        Text(status.text, style = Type.footnote, fontWeight = FontWeight.SemiBold, color = status.color)
    }
}

@Composable
fun Dot(color: Color, size: Dp = 8.dp) {
    Box(Modifier.size(size).background(color, CircleShape))
}

/** A white (or dark grey) rounded card on the grouped background. */
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
}

/** A titled inset group, like a grouped list's section. */
@Composable
fun InsetGroup(title: String, content: @Composable () -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(title.uppercase(), Modifier.padding(horizontal = 16.dp), style = Type.footnote, color = palette.secondary)
        Column(Modifier.fillMaxWidth().card(12.dp, palette.card)) { content() }
    }
}

@Composable
fun ListRow(
    symbol: ImageVector?, tint: Color, title: String, subtitle: String?, trailing: String, first: Boolean,
    tag: String? = null, onClick: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 44.dp).clickable(onClick = onClick).then(if (tag != null) Modifier.testTag(tag) else Modifier)
            .padding(start = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (symbol != null) {
            Box(Modifier.width(26.dp), Alignment.Center) { Icon(symbol, null, Modifier.size(22.dp), tint = tint) }
            Spacer(Modifier.width(14.dp))
        }
        Column(Modifier.weight(1f)) {
            if (!first) HorizontalDivider(thickness = 0.5.dp, color = palette.separator)
            Row(Modifier.padding(top = 11.dp, bottom = 11.dp, end = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                    Text(title, style = Type.body, color = palette.label, maxLines = 1)
                    subtitle?.let { Text(it, style = Type.subheadline, color = palette.secondary, maxLines = 1) }
                }
                if (trailing.isNotEmpty()) Text(trailing, Modifier.padding(start = 8.dp), style = Type.body, color = palette.secondary)
                Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, null, Modifier.size(22.dp), tint = palette.tertiary)
            }
        }
    }
}

/**
 * A pending approval as a card, with its countdown. A permission can be approved with a
 * fingerprint or the screen lock, or denied; a question or plan can only be denied here, since
 * approving would leave the agent at a dialog nobody can answer from the phone.
 */
@Composable
fun ApprovalCard(model: AppModel, approval: Approval, session: Session, open: (() -> Unit)? = null, decided: suspend () -> Unit = {}) {
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    var result by remember { mutableStateOf<String?>(null) }
    var now by remember { mutableLongStateOf(Countdown.nowMs()) }
    LaunchedEffect(approval.id) {
        while (true) {
            now = Countdown.nowMs()
            delay(1_000)
        }
    }
    val title = when (approval.kind) {
        "question" -> "${session.label} has a question"
        "plan" -> "${session.label} has a plan for you"
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

    Column(
        Modifier.fillMaxWidth().card(20.dp, palette.card).padding(16.dp).testTag("approval-${approval.id}"),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(
            Modifier.then(if (open != null) Modifier.clickable(onClick = open) else Modifier),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Box(Modifier.size(40.dp).card(11.dp, palette.needs.copy(alpha = 0.14f)), Alignment.Center) {
                Icon(
                    if (approval.kind == "permission") Icons.Outlined.VerifiedUser else Icons.AutoMirrored.Outlined.HelpOutline,
                    null, Modifier.size(22.dp), tint = palette.needs,
                )
            }
            Column(Modifier.weight(1f)) {
                Text(title, style = Type.headline, color = palette.label)
                Text("Denied automatically in ${Countdown.text(approval.deadline, now)}", style = Type.footnote, color = palette.needs)
            }
            if (open != null) Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, null, Modifier.size(22.dp), tint = palette.tertiary)
        }
        when (approval.kind) {
            "question" -> {
                approval.questions.forEach { Text(it, style = Type.body, color = palette.label) }
                Text("Answer questions in Herdr for now.", style = Type.footnote, color = palette.secondary)
            }
            "plan" -> {
                Text(approval.plan.orEmpty(), style = Type.callout, color = palette.label)
                Text("Approve plans in Herdr for now.", style = Type.footnote, color = palette.secondary)
            }
            else -> SelectionContainer {
                Text(
                    approval.inputText,
                    Modifier.fillMaxWidth().card(12.dp, codeBackground).padding(horizontal = 14.dp, vertical = 12.dp),
                    color = Color.White, fontFamily = FontFamily.Monospace, fontSize = 15.sp, lineHeight = 20.sp,
                )
            }
        }
        result?.let { Text(it, style = Type.footnote, color = palette.secondary) }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            BigButton("Deny", null, palette.fill, palette.label, !busy, Modifier.weight(1f).testTag("deny-${approval.id}")) {
                model.hub?.let { hub -> decide { hub.deny(approval.id, UUID.randomUUID().toString()) } }
            }
            if (approval.kind == "permission") {
                BigButton("Approve", Icons.Filled.Fingerprint, palette.accent, Color.White, !busy, Modifier.weight(1f).testTag("approve-${approval.id}")) {
                    model.hub?.let { hub ->
                        decide { hub.approve(approval.id, "Approve ${approval.toolName} in ${session.label}", UUID.randomUUID().toString()) }
                    }
                }
            }
        }
    }
}

@Composable
private fun BigButton(
    text: String, icon: ImageVector?, background: Color, foreground: Color, enabled: Boolean, modifier: Modifier, onClick: () -> Unit,
) {
    Row(
        modifier.height(50.dp).card(14.dp, background).clickable(enabled = enabled, onClick = onClick),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        icon?.let { Icon(it, null, Modifier.padding(end = 6.dp).size(20.dp), tint = foreground) }
        Text(text, style = Type.headline, color = if (enabled) foreground else foreground.copy(alpha = 0.4f))
    }
}

private fun describe(r: DecisionResponse): String = when {
    r.replayed -> "Already decided: ${r.outcome}."
    r.outcome == "allow" -> if (r.delivery == "unknown") "Approved; the hub lost the connection, so it may have run." else "Approved."
    else -> "Denied."
}

/** The last lines of an agent's terminal: its own words, untrusted. */
@Composable
fun OutputBlock(text: String, lines: Int = 14, tag: String = "output") {
    val tail = text.split("\n").map { if (it.isBlank()) "" else it }.dropLastWhile { it.isEmpty() }.takeLast(lines).joinToString("\n")
    SelectionContainer {
        Text(
            tail.ifEmpty { "No output yet." },
            Modifier.fillMaxWidth().card(16.dp, codeBackground).padding(14.dp).testTag(tag),
            color = codeText, fontFamily = FontFamily.Monospace, fontSize = 12.5.sp, lineHeight = 16.sp,
        )
    }
}

/** The whole tail of an agent's terminal, selectable, opened from "Show the full output". */
@Composable
fun FullOutput(title: String, text: String, dismiss: () -> Unit) {
    Dialog(dismiss, DialogProperties(usePlatformDefaultWidth = false)) {
        Column(Modifier.fillMaxSize().background(codeBackground)) {
            Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(title, Modifier.weight(1f), style = Type.headline, color = Color.White, maxLines = 1)
                Text("Done", Modifier.clickable(onClick = dismiss), style = Type.headline, color = palette.accent)
            }
            val scroll = rememberScrollState()
            LaunchedEffect(text) { scroll.scrollTo(scroll.maxValue) }
            SelectionContainer(Modifier.verticalScroll(scroll)) {
                Text(
                    text.ifEmpty { "No output yet." },
                    Modifier.padding(16.dp), color = codeText, fontFamily = FontFamily.Monospace, fontSize = 12.sp,
                )
            }
        }
    }
}

/** What this phone last said there, kept on the phone. */
@Composable
fun SaidBubble(said: AppModel.Said) {
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(
            said.text,
            Modifier.card(18.dp, palette.accent).padding(horizontal = 13.dp, vertical = 9.dp),
            style = Type.body, color = Color.White,
        )
        Text("You said · ${DateUtils.getRelativeTimeSpanString(said.at)}", style = Type.caption, color = palette.secondary)
    }
}

/** Changed files, laid out like attachments. */
@Composable
fun ChangesGrid(changes: Changes) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        val n = changes.files.size
        Text(
            (if (n == 1) "1 changed file" else "$n${if (changes.truncated) "+" else ""} changed files").uppercase(),
            style = Type.footnote, fontWeight = FontWeight.SemiBold, color = palette.secondary,
        )
        changes.files.chunked(2).forEach { pair ->
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                pair.forEach { f ->
                    Row(
                        Modifier.weight(1f).card(12.dp, palette.card).padding(12.dp).testTag(f.path),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Icon(Icons.Outlined.Description, null, Modifier.size(24.dp), tint = palette.secondary)
                        Column {
                            Text(f.path.substringAfterLast('/'), style = Type.subheadline, fontWeight = FontWeight.SemiBold, color = palette.label, maxLines = 1)
                            if (f.added == null && f.deleted == null) {
                                Text("new", style = Type.footnote, color = palette.done)
                            } else {
                                Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                                    Text("+${f.added ?: 0}", style = Type.footnote, color = palette.done)
                                    Text("−${f.deleted ?: 0}", style = Type.footnote, color = palette.red)
                                }
                            }
                        }
                    }
                }
                if (pair.size == 1) Spacer(Modifier.weight(1f))
            }
        }
    }
}
