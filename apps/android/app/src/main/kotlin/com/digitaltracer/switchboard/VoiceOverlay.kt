package com.digitaltracer.switchboard

import android.Manifest
import android.content.pm.PackageManager
import android.view.HapticFeedbackConstants
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.blur
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitaltracer.switchboard.api.models.TargetView
import kotlin.math.abs
import kotlin.math.sin

/** Starts a listen for a target, keeping the text so far or not. */
fun interface Listen {
    operator fun invoke(target: TargetView?, keepText: Boolean)
}

/** Starts a listen, asking for the microphone the first time. */
@Composable
fun rememberListen(voice: Voice): Listen {
    val context = LocalContext.current
    val view = LocalView.current
    // Who the listen is for while the permission prompt is up.
    val pending = remember { arrayOfNulls<TargetView>(1) }
    val keep = remember { BooleanArray(1) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) voice.listen(pending[0], keep[0]) else voice.denied()
    }
    return remember(voice) {
        Listen { target, keepText ->
            view.performHapticFeedback(HapticFeedbackConstants.CONTEXT_CLICK)
            if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
                voice.listen(target, keepText)
            } else {
                pending[0] = target
                keep[0] = keepText
                ask.launch(Manifest.permission.RECORD_AUDIO)
            }
        }
    }
}

/** Everything voice mode draws over the app: the listening page and the edge glow. */
@Composable
fun VoiceOverlay(voice: Voice) {
    val listening = voice.phase == Voice.Phase.Listening
    val shown = listening || voice.phase == Voice.Phase.Transcribing
    Box(Modifier.fillMaxSize()) {
        AnimatedVisibility(
            shown,
            enter = fadeIn(tween(300)) + scaleIn(tween(300), 0.96f) + slideInVertically(tween(300)) { it / 8 },
            exit = fadeOut(tween(200)) + scaleOut(tween(200), 0.96f),
        ) {
            ListeningPanel(voice)
        }
        EdgeGlow(listening, voice.level)
    }
}

/** What's being heard, full screen: the words as they come, a level meter, then Cancel, Done and Type instead. */
@Composable
private fun ListeningPanel(voice: Voice) {
    Column(
        Modifier.fillMaxSize().background(palette.sf).clickable(remember { MutableInteractionSource() }, null) {}
            .windowInsetsPadding(WindowInsets.systemBars).padding(horizontal = 20.dp).padding(bottom = 12.dp),
    ) {
        Row(Modifier.height(44.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Dot(palette.bad)
            Text(if (voice.cloud) "Recording for OpenAI" else "Listening on this phone", style = Type.meta.copy(fontWeight = FontWeight.Medium), color = palette.sub)
        }
        Text(
            (if (voice.dictating) "Dictating the prompt" else voice.target?.let { "To ${it.label}" } ?: "Switchboard picks the agent").uppercase(),
            Modifier.padding(top = 36.dp), style = Type.section, color = palette.mut,
        )
        Column(Modifier.weight(1f).padding(top = 8.dp).verticalScroll(rememberScrollState())) { Transcript(voice) }
        voice.notice?.let { Note(it, Modifier.padding(bottom = 12.dp).testTag("voiceNotice")) }
        if (voice.phase == Voice.Phase.Transcribing) {
            Row(Modifier.fillMaxWidth().padding(bottom = 28.dp), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.size(20.dp), color = palette.mut, strokeWidth = 2.dp)
                Text("Transcribing with OpenAI…", Modifier.padding(start = 10.dp), style = Type.sub, color = palette.sub)
            }
            RoundIcon(Icons.Filled.Close, "Cancel") { voice.cancel() }
        } else {
            Wave(voice.level)
            Row(Modifier.fillMaxWidth().padding(top = 28.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                RoundIcon(Icons.Filled.Close, "Cancel") { voice.cancel() }
                BigButton("Done", Modifier.weight(1f).testTag("voiceDone"), shape = RoundedCornerShape(26.dp)) { voice.stop() }
                RoundIcon(Icons.Outlined.Keyboard, "Type instead") { voice.typeInstead() }
            }
        }
    }
}

/** Bars that follow how loud it is. */
@Composable
private fun Wave(level: Float) {
    val t by rememberInfiniteTransition(label = "wave").animateFloat(
        0f, 1000f, infiniteRepeatable(tween(1_000_000, easing = LinearEasing), RepeatMode.Restart), label = "t",
    )
    val shape = listOf(0.2f, 0.35f, 0.65f, 0.95f, 0.55f, 0.8f, 1f, 0.45f, 0.7f, 0.9f, 0.4f, 0.6f, 0.25f, 0.15f)
    Row(Modifier.fillMaxWidth().height(40.dp), horizontalArrangement = Arrangement.spacedBy(4.dp, Alignment.CenterHorizontally), verticalAlignment = Alignment.CenterVertically) {
        shape.forEachIndexed { i, h ->
            val wave = 0.6f + 0.4f * abs(sin(t * 7 + i * 1.3f))
            Box(Modifier.width(4.dp).height((6 + 34 * h * minOf(1f, 0.25f + level * 1.6f) * wave).dp).background(palette.ink, CircleShape))
        }
    }
}

/** Finished words in full colour, the one still being recognised in grey. */
@Composable
private fun Transcript(voice: Voice) {
    val words = voice.heard.split(" ").filter { it.isNotEmpty() }
    val style = Type.state.copy(fontSize = 38.sp, lineHeight = 41.sp)
    val text = if (voice.cloud) {
        buildAnnotatedString {
            if (voice.text.isNotEmpty()) {
                withStyle(SpanStyle(color = palette.ink)) { append(voice.text + "\n") }
            }
            withStyle(SpanStyle(color = palette.mut)) {
                append(if (voice.phase == Voice.Phase.Transcribing) "Transcribing with OpenAI…" else "Recording for OpenAI…")
            }
        }
    } else if (words.isEmpty()) {
        buildAnnotatedString {
            withStyle(SpanStyle(color = if (voice.text.isEmpty()) palette.mut else palette.ink)) {
                append(voice.text.ifEmpty { "Listening…" })
            }
        }
    } else {
        val done = (listOf(voice.text) + words.dropLast(1)).filter { it.isNotEmpty() }.joinToString(" ")
        buildAnnotatedString {
            withStyle(SpanStyle(color = palette.ink)) { append(if (done.isEmpty()) "" else "$done ") }
            withStyle(SpanStyle(color = palette.mut)) { append(words.last()) }
        }
    }
    Text(text, Modifier.fillMaxWidth().heightIn(min = 60.dp).testTag("transcript"), style = style)
}

/** The light along the screen's edge while listening, louder speech brighter. */
@Composable
private fun EdgeGlow(listening: Boolean, level: Float) {
    val shown by animateFloatAsState(if (listening) 1f else 0f, tween(if (listening) 400 else 450), label = "glow")
    if (shown == 0f) return
    val turn by rememberInfiniteTransition(label = "turn").animateFloat(
        0f, 360f, infiniteRepeatable(tween(4_000, easing = LinearEasing)), label = "angle",
    )
    val colors = listOf(Color(0xFF8C5CF5), Color(0xFFFF4D8C), Color(0xFFFFA24D), Color(0xFF4DA3FF), Color(0xFF8C5CF5))
    Canvas(Modifier.fillMaxSize().blur(14.dp).alpha(shown * (0.65f + 0.35f * level))) {
        val width = 10.dp.toPx() + 8.dp.toPx() * level
        drawRoundRect(
            Brush.sweepGradient(colors, Offset(size.width / 2, size.height / 2)),
            size = size,
            cornerRadius = CornerRadius(40.dp.toPx()),
            style = Stroke(width),
            alpha = 1f,
        )
        drawRoundRect(
            Brush.linearGradient(colors, start = Offset(0f, size.height * (turn / 360f)), end = Offset(size.width, size.height * (1 - turn / 360f))),
            size = size,
            cornerRadius = CornerRadius(40.dp.toPx()),
            style = Stroke(width / 2),
            alpha = 0.6f,
        )
    }
}
