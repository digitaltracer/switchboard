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
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.scale
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
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

/** The mic, which fades into the listening panel while voice mode is on. */
@Composable
fun MicButton(voice: Voice, size: Dp, label: String, tag: String, enabled: Boolean = true, onClick: () -> Unit) {
    val listening = voice.phase == Voice.Phase.Listening
    val shown by animateFloatAsState(if (listening) 0f else 1f, tween(if (listening) 200 else 250, if (listening) 0 else 200), label = "mic")
    Box(
        Modifier.size(size).scale(0.7f + 0.3f * shown).alpha(shown * if (enabled) 1f else 0.4f)
            .shadow(12.dp, CircleShape, ambientColor = palette.accent, spotColor = palette.accent)
            .background(palette.accent, CircleShape)
            .clickable(enabled = enabled, onClick = onClick)
            .semantics { contentDescription = label }.testTag(tag),
        Alignment.Center,
    ) {
        Icon(Icons.Filled.Mic, null, Modifier.size(size * 0.42f), tint = Color.White)
    }
}

@Composable
fun CircleButton(icon: ImageVector, label: String, size: Dp = 44.dp, background: Color = palette.fill, tag: String? = null, onClick: () -> Unit) {
    Box(
        Modifier.size(size).background(background, CircleShape).clickable(onClick = onClick)
            .semantics { contentDescription = label }.then(if (tag != null) Modifier.testTag(tag) else Modifier),
        Alignment.Center,
    ) {
        Icon(icon, null, Modifier.size(22.dp), tint = palette.label)
    }
}

/** Everything voice mode draws over the app: the edge glow and the listening panel. */
@Composable
fun VoiceOverlay(voice: Voice) {
    val listening = voice.phase == Voice.Phase.Listening
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        if (listening) {
            Box(Modifier.fillMaxSize().clickable(remember { MutableInteractionSource() }, null) { voice.cancel() })
        }
        EdgeGlow(listening, voice.level)
        AnimatedVisibility(
            listening,
            enter = fadeIn(tween(300)) + scaleIn(tween(300), 0.9f) + slideInVertically(tween(300)) { it / 4 },
            exit = fadeOut(tween(200)) + scaleOut(tween(200), 0.9f) + slideOutVertically(tween(200)) { it / 4 },
        ) {
            ListeningPanel(voice)
        }
    }
}

/** What's being heard, with Done to stop listening, ✕ to cancel and the keyboard to type instead. */
@Composable
private fun ListeningPanel(voice: Voice) {
    Column(
        Modifier.windowInsetsPadding(WindowInsets.navigationBars).padding(10.dp).fillMaxWidth()
            .shadow(20.dp, RoundedCornerShape(38.dp))
            .background(palette.card, RoundedCornerShape(38.dp))
            .clickable(remember { MutableInteractionSource() }, null) {}
            .padding(start = 20.dp, end = 20.dp, top = 20.dp, bottom = 16.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Text(
            voice.target?.let { "To ${it.label}" } ?: "Switchboard picks the agent",
            style = Type.footnote, fontWeight = FontWeight.SemiBold, color = palette.secondary,
        )
        Transcript(voice)
        Row(verticalAlignment = Alignment.CenterVertically) {
            CircleButton(Icons.Filled.Close, "Cancel") { voice.cancel() }
            Spacer(Modifier.weight(1f))
            Row(
                Modifier.height(44.dp).background(Color(0xFF1C1C1C), CircleShape).clickable { voice.stop() }
                    .padding(start = 16.dp, end = 20.dp).testTag("voiceDone"),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                LevelBars(voice.level)
                Text("Done", style = Type.headline, color = Color.White)
            }
            Spacer(Modifier.weight(1f))
            CircleButton(Icons.Outlined.Keyboard, "Type instead") { voice.typeInstead() }
        }
    }
}

/** Finished words in full colour, the one still being recognised in grey. */
@Composable
private fun Transcript(voice: Voice) {
    val words = voice.heard.split(" ").filter { it.isNotEmpty() }
    val style = Type.title3.copy(fontSize = 22.sp, lineHeight = 28.sp, fontWeight = FontWeight.SemiBold)
    val text = if (words.isEmpty()) {
        buildAnnotatedString {
            withStyle(SpanStyle(color = if (voice.text.isEmpty()) palette.secondary else palette.label)) {
                append(voice.text.ifEmpty { "Listening…" })
            }
        }
    } else {
        val done = (listOf(voice.text) + words.dropLast(1)).filter { it.isNotEmpty() }.joinToString(" ")
        buildAnnotatedString {
            withStyle(SpanStyle(color = palette.label)) { append(if (done.isEmpty()) "" else "$done ") }
            withStyle(SpanStyle(color = palette.secondary)) { append(words.last()) }
        }
    }
    Text(text, Modifier.fillMaxWidth().heightIn(min = 60.dp).testTag("transcript"), style = style)
}

@Composable
fun LevelBars(level: Float) {
    val t by rememberInfiniteTransition(label = "bars").animateFloat(
        0f, 1000f, infiniteRepeatable(tween(1_000_000, easing = LinearEasing), RepeatMode.Restart), label = "t",
    )
    val brush = Brush.verticalGradient(listOf(Color(0xFF8C5CF5), Color(0xFFFF4D8C)))
    Row(Modifier.height(20.dp), horizontalArrangement = Arrangement.spacedBy(3.dp), verticalAlignment = Alignment.CenterVertically) {
        repeat(5) { i ->
            val wave = 0.55f + 0.45f * abs(sin(t * 9 + i * 1.7f))
            val edge = if (i == 0 || i == 4) 0.6f else 1f
            Box(Modifier.width(3.dp).height((4 + 16 * minOf(1f, level * wave * edge)).dp).background(brush, CircleShape))
        }
    }
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
