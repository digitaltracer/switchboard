package com.digitaltracer.switchboard

import android.provider.Settings
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.core.HubError

/** What home says before the hub's first answer: connecting, or which step failed and why. */
class ConnectState(val main: String, val quiet: String?, val explanation: String?, val steps: List<Step>) {
    enum class Kind { Ok, Failed, Unreached }
    class Step(val name: String, val detail: String, val kind: Kind)

    companion object {
        fun of(model: AppModel, slow: Boolean): ConnectState {
            val relay = model.relayHost ?: "relay"
            if (!model.unreachable) {
                return ConnectState(
                    "Connecting to your hub.",
                    if (slow) "This is taking a while. A weak signal slows it down." else "Your agents show here once it answers.",
                    null, emptyList(),
                )
            }
            val phone = Step("This phone", "Online", Kind.Ok)
            return when {
                model.reach == HubError.Reach.Offline -> ConnectState(
                    "This phone is offline.", "Still trying.",
                    "Join Wi-Fi or turn on mobile data. Switchboard reconnects by itself, and your agents keep working meanwhile.",
                    listOfNotNull(
                        Step("This phone", "No network", Kind.Failed),
                        if (model.viaRelay) Step("Relay", relay, Kind.Unreached) else null,
                        Step("Your hub", "", Kind.Unreached),
                    ),
                )
                model.reach == HubError.Reach.Hub -> ConnectState(
                    "Your hub is offline.", "Still trying.",
                    "The relay is up, but your hub isn't connected to it. Check that the hub's machine is awake and the hub is running.",
                    listOf(phone, Step("Relay", relay, Kind.Ok), Step("Your hub", "Not connected", Kind.Failed)),
                )
                model.reach == HubError.Reach.Slow -> ConnectState(
                    "Your hub isn't answering.", "Still trying.",
                    "It's connected but slow to reply, usually because its machine is busy or on a weak network. It tends to catch up.",
                    listOf(phone, Step("Relay", relay, Kind.Ok), Step("Your hub", "Slow to answer", Kind.Failed)),
                )
                model.viaRelay -> ConnectState(
                    "Can’t reach the relay.", "Still trying.",
                    "A weak signal can do this, or the relay may be down for a moment. Your agents keep working on your hub meanwhile.",
                    listOf(phone, Step("Relay", "Not answering", Kind.Failed), Step("Your hub", "", Kind.Unreached)),
                )
                else -> ConnectState(
                    "Can’t reach your hub.", "Still trying.",
                    "Check this phone's connection, and that the hub's machine is on and the hub is running.",
                    listOf(phone, Step("Your hub", "Not answering", Kind.Failed)),
                )
            }
        }
    }
}

/** In place of the lists while the hub can't be reached: the path, what to check, and Try again. */
@Composable
fun ColumnScope.HubUnreachable(model: AppModel, state: ConnectState) {
    Column(Modifier.weight(1f).fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 16.dp).testTag("hubUnreachable")) {
        Spacer(Modifier.height(30.dp))
        ReachPath(state.steps)
        state.explanation?.let {
            Spacer(Modifier.height(30.dp))
            Text(it, style = Type.field, color = palette.sub)
        }
        Spacer(Modifier.weight(1f))
        BigButton(
            if (model.trying) "Trying…" else "Try again", Modifier.fillMaxWidth().testTag("tryAgain"),
            kind = ButtonKind.Outline, enabled = !model.trying,
        ) { model.startPolling() }
        Text(
            "Switchboard also tries again by itself every few seconds.", Modifier.fillMaxWidth().padding(top = 10.dp),
            style = Type.meta, color = palette.mut, textAlign = TextAlign.Center,
        )
    }
}

/** Whether the system's animations are off, as Android's version of Reduce Motion. */
@Composable
private fun reduceMotion(): Boolean {
    val resolver = LocalContext.current.contentResolver
    return remember { Settings.Global.getFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
}

/**
 * Phone → relay → hub as dots on a line: green up to the step that failed, orange there, grey after.
 * A pulse travels from the phone and stops at the break.
 */
@Composable
fun ReachPath(steps: List<ConnectState.Step>) {
    val still = reduceMotion()
    val clock = rememberInfiniteTransition(label = "reach")
    val run by clock.animateFloat(0f, 1f, infiniteRepeatable(tween(2200, easing = LinearEasing)), label = "pulse")
    val beat by clock.animateFloat(0f, 1f, infiniteRepeatable(tween(1000), RepeatMode.Reverse), label = "beat")
    val ok = palette.ok
    val warn = palette.warn
    val line = palette.line
    val bg = palette.bg
    Column(Modifier.fillMaxWidth().clearAndSetSemantics { contentDescription = steps.joinToString(", ") { "${it.name}: ${it.detail}" } }) {
        Canvas(Modifier.fillMaxWidth().height(22.dp)) {
            val r = 12.dp.toPx()
            val y = size.height / 2
            fun x(i: Int) = if (steps.size < 2) r else r + (size.width - 2 * r) * i / (steps.size - 1)
            val failed = steps.indexOfFirst { it.kind == ConnectState.Kind.Failed }.takeIf { it >= 0 } ?: (steps.size - 1)
            for (i in 0 until steps.size - 1) {
                val reached = i + 1 < failed
                val broken = i + 1 == failed
                drawLine(
                    if (reached) ok.copy(alpha = 0.55f) else if (broken) warn.copy(alpha = 0.7f) else line,
                    Offset(x(i), y), Offset(x(i + 1), y), 2.dp.toPx(), StrokeCap.Round,
                    if (reached) null else PathEffect.dashPathEffect(floatArrayOf(3.dp.toPx(), 5.dp.toPx())),
                )
            }
            if (failed > 0 && !still) {
                val px = x(0) + (x(failed) - x(0)) * minOf(run / 0.8f, 1f)
                val fade = if (run < 0.8f) 1f else 1 - (run - 0.8f) / 0.2f
                drawCircle(ok.copy(alpha = 0.9f * fade), 4.dp.toPx(), Offset(px, y))
            }
            steps.forEachIndexed { i, s ->
                val c = Offset(x(i), y)
                when (s.kind) {
                    ConnectState.Kind.Ok -> drawCircle(ok, 7.dp.toPx(), c)
                    ConnectState.Kind.Failed -> {
                        drawCircle(warn.copy(alpha = 0.12f + 0.14f * (if (still) 0f else beat)), 11.dp.toPx(), c)
                        drawCircle(warn, 7.dp.toPx(), c)
                    }
                    ConnectState.Kind.Unreached -> {
                        drawCircle(bg, 7.dp.toPx(), c)
                        drawCircle(line, 6.dp.toPx(), c, style = Stroke(2.dp.toPx()))
                    }
                }
            }
        }
        Spacer(Modifier.height(12.dp))
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            steps.forEachIndexed { i, s ->
                val align = when (i) {
                    0 -> Alignment.Start
                    steps.size - 1 -> Alignment.End
                    else -> Alignment.CenterHorizontally
                }
                Column(Modifier.weight(1f), horizontalAlignment = align) {
                    Text(s.name, style = Type.text.copy(fontWeight = FontWeight.Medium), color = if (s.kind == ConnectState.Kind.Unreached) palette.mut else palette.ink)
                    Text(
                        s.detail.ifEmpty { " " }, style = Type.meta,
                        color = if (s.kind == ConnectState.Kind.Failed) palette.warn else palette.mut,
                        maxLines = 1, overflow = TextOverflow.Ellipsis,
                    )
                }
            }
        }
    }
}

/** Grey rows in a list's place before its first answer: the shape of what's coming, never made-up names. */
@Composable
fun LoadingRows() {
    val still = reduceMotion()
    val pulse by rememberInfiniteTransition(label = "loading").animateFloat(1f, 0.4f, infiniteRepeatable(tween(900), RepeatMode.Reverse), label = "dim")
    val widths = listOf(0.42f to 0.7f, 0.55f to 0.48f, 0.36f to 0.62f, 0.48f to 0.4f)
    Column(Modifier.fillMaxWidth().alpha(if (still) 1f else pulse).clearAndSetSemantics { contentDescription = "Loading" }.testTag("loadingRows")) {
        widths.forEachIndexed { i, (title, sub) ->
            if (i > 0) Hairline()
            Row(Modifier.fillMaxWidth().height(60.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(12.dp).background(palette.fill, CircleShape))
                Spacer(Modifier.size(12.dp))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Box(Modifier.fillMaxWidth(title).height(13.dp).background(palette.fill, RoundedCornerShape(50)))
                    Box(Modifier.fillMaxWidth(sub).height(10.dp).background(palette.fill, RoundedCornerShape(50)))
                }
            }
        }
    }
}
