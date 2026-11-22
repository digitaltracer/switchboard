package com.digitaltracer.switchboard

import android.provider.Settings
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBackIos
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.outlined.Home
import androidx.compose.material.icons.outlined.Mic
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/** Status, never by colour alone: each has its own glyph. */
enum class Glyph {
    Need, Work, Done, Idle, End;

    companion object {
        fun of(agentStatus: String) = when (agentStatus) {
            "blocked" -> Need
            "working" -> Work
            "done" -> Done
            "ended", "stopped", "exited", "failed", "deleted" -> End
            else -> Idle
        }
    }
}

@Composable
fun StatusGlyph(glyph: Glyph, size: Dp = 18.dp) {
    val p = palette
    val turn = if (glyph == Glyph.Work) spin() else 0f
    Canvas(Modifier.size(size)) {
        val s = this.size.minDimension
        val w = s * 1.7f / 18f
        when (glyph) {
            Glyph.Need -> rotate(45f) {
                val side = s * 0.76f
                drawRoundRect(p.warn, Offset((s - side) / 2, (s - side) / 2), Size(side, side), CornerRadius(s * 4 / 18f))
            }
            Glyph.Work -> drawArc(p.ac, -135f + turn, 270f, false, Offset(w / 2, w / 2), Size(s - w, s - w), style = Stroke(w, cap = StrokeCap.Round))
            Glyph.Done -> {
                drawCircle(p.ok, (s - w) / 2, style = Stroke(w))
                val path = Path().apply {
                    moveTo(s * 0.30f, s * 0.52f); lineTo(s * 0.45f, s * 0.66f); lineTo(s * 0.70f, s * 0.36f)
                }
                drawPath(path, p.ok, style = Stroke(w, cap = StrokeCap.Round, join = StrokeJoin.Round))
            }
            Glyph.Idle -> drawCircle(p.mut, (s - w) / 2, style = Stroke(w))
            Glyph.End -> drawCircle(
                p.mut, (s - w) / 2,
                style = Stroke(w, pathEffect = PathEffect.dashPathEffect(floatArrayOf(s * 0.16f, s * 0.12f))),
            )
        }
    }
}

/** One turn a second, in degrees; still when the system's animations are off. */
@Composable
private fun spin(): Float {
    val context = LocalContext.current
    val off = remember { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
    if (off) return 0f
    val turn by rememberInfiniteTransition(label = "work").animateFloat(0f, 360f, infiniteRepeatable(tween(1000, easing = LinearEasing)), label = "turn")
    return turn
}

/** Where an agent lives: the Herdr app or a Herdr terminal, by shape as well as word. */
@Composable
fun WhereTag(backend: String) {
    val app = backend == "hc"
    Row(
        Modifier.height(22.dp).clip(RoundedCornerShape(7.dp)).background(palette.sf)
            .border(1.dp, palette.line, RoundedCornerShape(7.dp)).padding(horizontal = 8.dp)
            .semantics { contentDescription = if (app) "In the Herdr app" else "In a Herdr terminal" },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        if (app) {
            val c = palette.sub
            Canvas(Modifier.size(width = 10.dp, height = 8.dp)) {
                drawRoundRect(c, style = Stroke(1.4.dp.toPx()), cornerRadius = CornerRadius(2.dp.toPx()))
                drawRect(c, size = Size(size.width, 3.dp.toPx()))
            }
        } else {
            Text(">_", style = Type.mono(10f, FontWeight.Medium), color = palette.sub)
        }
        Text(if (app) "App" else "Terminal", style = Type.meta.copy(fontWeight = FontWeight.Medium), color = palette.sub, maxLines = 1)
    }
}

/** "Working", "Needs you" as a small rounded chip with its glyph. */
@Composable
fun StatusChip(status: SessionStatus) {
    val need = status.glyph == Glyph.Need
    Row(
        Modifier.height(28.dp).clip(CircleShape).background(if (need) palette.wbg else palette.fill).padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        StatusGlyph(status.glyph, 13.dp)
        Text(
            status.text, style = Type.meta.copy(fontWeight = FontWeight.Medium), maxLines = 1,
            color = if (need) palette.warn else if (status.glyph == Glyph.Work) palette.ac else palette.sub,
        )
    }
}

@Composable
fun Chip(text: String, on: Boolean = false, modifier: Modifier = Modifier) {
    Box(modifier.height(28.dp).clip(CircleShape).background(if (on) palette.ink else palette.fill).padding(horizontal = 10.dp), Alignment.Center) {
        Text(text, style = Type.meta.copy(fontWeight = FontWeight.Medium), color = if (on) palette.inv else palette.sub, maxLines = 1)
    }
}

/** The uppercase label over a list, with an optional count or link at its end. */
@Composable
fun SectionHead(title: String, trailing: String? = null, top: Dp = 22.dp, action: (@Composable () -> Unit)? = null) {
    Row(Modifier.fillMaxWidth().padding(top = top, bottom = 8.dp).semantics { heading() }, verticalAlignment = Alignment.CenterVertically) {
        Text(title.uppercase(), Modifier.weight(1f), style = Type.section, color = palette.mut, maxLines = 1)
        trailing?.let { Text(it, style = Type.section, color = palette.mut) }
        action?.invoke()
    }
}

@Composable
fun Hairline(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth().height(1.dp).background(palette.line))
}

@Composable
fun Chevron(down: Boolean = false) {
    Icon(
        if (down) Icons.Filled.KeyboardArrowDown else Icons.AutoMirrored.Filled.KeyboardArrowRight, null,
        Modifier.size(18.dp), tint = palette.mut,
    )
}

/** A flat row with a hairline above it, as every list draws them. */
@Composable
fun InboxRow(
    title: String,
    subtitle: String? = null,
    modifier: Modifier = Modifier,
    subtitleMono: Boolean = false,
    subtitleColor: Color? = null,
    dim: Boolean = false,
    first: Boolean = false,
    onClick: (() -> Unit)? = null,
    leading: (@Composable () -> Unit)? = null,
    trailing: (@Composable RowScope.() -> Unit)? = null,
) {
    Column(modifier.fillMaxWidth().then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)) {
        if (!first) Hairline()
        Row(Modifier.fillMaxWidth().heightIn(min = 60.dp).padding(vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            leading?.let { it(); Spacer(Modifier.width(12.dp)) }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                Text(
                    title, style = if (dim) Type.field else Type.title, color = if (dim) palette.mut else palette.ink,
                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
                if (!subtitle.isNullOrEmpty()) {
                    Text(
                        subtitle, style = if (subtitleMono) Type.mono(12.5f) else Type.sub,
                        color = if (dim) palette.mut else subtitleColor ?: palette.sub, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            trailing?.let { Spacer(Modifier.width(12.dp)); it() }
        }
    }
}

/** A label and its value on one line, as Details and At your desk show them. */
@Composable
fun FieldRow(label: String, value: String, mono: Boolean = false, first: Boolean = false, valueContent: (@Composable () -> Unit)? = null) {
    Column(Modifier.fillMaxWidth()) {
        if (!first) Hairline()
        Row(Modifier.fillMaxWidth().heightIn(min = 50.dp).padding(vertical = 11.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(label, style = Type.field, color = palette.ink)
            Spacer(Modifier.width(12.dp).weight(1f))
            if (valueContent != null) valueContent() else {
                Text(value, style = if (mono) Type.mono(12.5f) else Type.field, color = palette.sub, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

enum class ButtonKind { Primary, Outline, Plain }

/** A 52-point button: black primary, outlined, or greyed when it can't be used yet. */
@Composable
fun BigButton(
    text: String,
    modifier: Modifier = Modifier,
    kind: ButtonKind = ButtonKind.Primary,
    enabled: Boolean = true,
    destructive: Boolean = false,
    icon: ImageVector? = null,
    busy: Boolean = false,
    shape: RoundedCornerShape = RoundedCornerShape(14.dp),
    color: Color? = null,
    onClick: () -> Unit,
) {
    val fg = when {
        !enabled && kind == ButtonKind.Primary -> palette.mut
        color != null -> color
        destructive -> palette.bad
        kind == ButtonKind.Primary -> palette.inv
        else -> palette.ink
    }
    val bg = when (kind) {
        ButtonKind.Primary -> if (enabled) palette.ink else palette.fill
        ButtonKind.Outline -> palette.sf
        ButtonKind.Plain -> Color.Transparent
    }
    Row(
        modifier.heightIn(min = 52.dp).clip(shape).background(bg)
            .then(if (kind == ButtonKind.Outline) Modifier.border(BorderStroke(1.5.dp, palette.line), shape) else Modifier)
            .clickable(enabled = enabled && !busy, role = Role.Button, onClick = onClick).padding(horizontal = 14.dp),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (busy) {
            CircularProgressIndicator(Modifier.size(20.dp), color = fg, strokeWidth = 2.dp)
        } else {
            icon?.let { Icon(it, null, Modifier.padding(end = 8.dp).size(20.dp), tint = fg) }
            Text(text, style = Type.button, color = if (!enabled && kind != ButtonKind.Primary) fg.copy(alpha = 0.5f) else fg, maxLines = 1)
        }
    }
}

/** The round black mic. */
@Composable
fun MicDisc(size: Dp = 40.dp, label: String, tag: String, enabled: Boolean = true, onClick: () -> Unit) {
    Box(
        Modifier.size(size).clip(CircleShape).background(if (enabled) palette.ink else palette.fill)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label }.testTag(tag),
        Alignment.Center,
    ) { Icon(Icons.Outlined.Mic, null, Modifier.size(size * 0.5f), tint = if (enabled) palette.inv else palette.mut) }
}

/** A 52-point outlined circle, for Cancel, Keep talking and Type instead. */
@Composable
fun RoundIcon(icon: ImageVector, label: String, tag: String? = null, onClick: () -> Unit) {
    Box(
        Modifier.size(52.dp).clip(CircleShape).border(1.5.dp, palette.line, CircleShape).clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label }.then(if (tag != null) Modifier.testTag(tag) else Modifier),
        Alignment.Center,
    ) { Icon(icon, null, Modifier.size(22.dp), tint = palette.ink) }
}

/** A card: the approval, the fingerprint. */
@Composable
fun CardBox(modifier: Modifier = Modifier, padding: Dp = 16.dp, content: @Composable ColumnScope.() -> Unit) {
    Column(
        modifier.fillMaxWidth().clip(RoundedCornerShape(18.dp)).background(palette.sf).border(1.dp, palette.line, RoundedCornerShape(18.dp))
            .padding(padding),
        verticalArrangement = Arrangement.spacedBy(12.dp),
        content = content,
    )
}

/** A command or input, white on near-black in both appearances. */
@Composable
fun CodeBlock(text: String, wraps: Boolean = false) {
    Text(
        text,
        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(palette.code).border(1.dp, palette.line.copy(alpha = 0.6f), RoundedCornerShape(10.dp))
            .padding(horizontal = 14.dp, vertical = 11.dp),
        style = Type.mono(13f), color = codeText, maxLines = if (wraps) Int.MAX_VALUE else 1, overflow = TextOverflow.Ellipsis,
    )
}

/** A warm notice: showing what was known, a notification that opened nothing. */
@Composable
fun Banner(text: String, modifier: Modifier = Modifier) {
    Text(
        text, modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(palette.wbg).padding(horizontal = 14.dp, vertical = 10.dp),
        style = Type.sub.copy(fontWeight = FontWeight.Medium), color = palette.warn,
    )
}

/** A radio row: a listed option, an "Open it in" choice, a destination. One not on offer is dashed and greyed. */
@Composable
fun RadioRow(
    title: String, on: Boolean, modifier: Modifier = Modifier, offered: Boolean = true, first: Boolean = false,
    onClick: (() -> Unit)? = null, detail: (@Composable () -> Unit)? = null,
) {
    Column(
        modifier.fillMaxWidth()
            .then(if (onClick != null) Modifier.clickable(enabled = offered, role = Role.RadioButton, onClick = onClick) else Modifier)
            .semantics { selected = on },
    ) {
        if (!first) Hairline()
        Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(vertical = 15.dp), verticalAlignment = Alignment.Top) {
            val ink = palette.ink
            val inv = palette.inv
            val mut = palette.mut
            Box(Modifier.size(22.dp), Alignment.Center) {
                Canvas(Modifier.size(22.dp)) {
                    val w = 1.7.dp.toPx()
                    if (on) {
                        drawCircle(ink)
                    } else {
                        drawCircle(
                            mut, (size.minDimension - w) / 2,
                            style = Stroke(w, pathEffect = if (offered) null else PathEffect.dashPathEffect(floatArrayOf(3.dp.toPx(), 3.dp.toPx()))),
                        )
                    }
                }
                if (on) Icon(Icons.Filled.Check, null, Modifier.size(13.dp), tint = inv)
            }
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(title, style = if (offered) Type.title else Type.field, color = if (offered) palette.ink else palette.mut)
                detail?.invoke()
            }
        }
    }
}

/** A quiet line. */
@Composable
fun Note(text: String, modifier: Modifier = Modifier, color: Color? = null) {
    Text(text, modifier, style = Type.meta.copy(lineHeight = Type.meta.fontSize * 1.45f), color = color ?: palette.mut)
}

/** The three text tabs: Now, Spaces, Projects; Summary, Conversation, Screen. */
@Composable
fun <T> TextTabs(tabs: List<Pair<T, String>>, selection: T, select: (T) -> Unit) {
    Column(Modifier.fillMaxWidth()) {
        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, top = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(22.dp),
        ) {
            tabs.forEach { (tab, label) ->
                val on = tab == selection
                Box(
                    Modifier.width(IntrinsicSize.Max).height(44.dp).clickable(role = Role.Tab) { select(tab) }
                        .semantics { selected = on }.testTag("tab-$label"),
                    Alignment.Center,
                ) {
                    Text(label, style = Type.text.copy(fontWeight = FontWeight.Medium), color = if (on) palette.ink else palette.mut, maxLines = 1, softWrap = false)
                    if (on) Box(Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(2.dp).background(palette.ink))
                }
            }
        }
        Hairline()
    }
}

/** Empties the page stack; null when one back already gets home. */
val LocalGoHome = staticCompositionLocalOf<(() -> Unit)?> { null }

/** The top line of a pushed page: back, and what it is on the right. A long press on back offers Home. */
@Composable
fun PageTop(back: String, onBack: () -> Unit, meta: String? = null, content: (@Composable RowScope.() -> Unit)? = null) {
    Row(
        Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.statusBars).height(44.dp).padding(start = 8.dp, end = 18.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val goHome = LocalGoHome.current
        var menu by remember { mutableStateOf(false) }
        Box(
            Modifier.size(44.dp)
                .combinedClickable(role = Role.Button, onLongClickLabel = goHome?.let { "Home" }, onLongClick = goHome?.let { { menu = true } }, onClick = onBack)
                .semantics { contentDescription = back }.testTag("back"),
            Alignment.Center,
        ) {
            Icon(Icons.AutoMirrored.Filled.ArrowBackIos, null, Modifier.size(19.dp).padding(start = 4.dp), tint = palette.ink)
            if (goHome != null) {
                DropdownMenu(menu, { menu = false }) {
                    DropdownMenuItem({ Text("Home", style = Type.field) }, {
                        menu = false
                        goHome()
                    }, Modifier.testTag("goHome"), leadingIcon = { Icon(Icons.Outlined.Home, null, tint = palette.ink) })
                }
            }
        }
        if (content != null) {
            Spacer(Modifier.width(4.dp))
            content()
        } else {
            Spacer(Modifier.weight(1f))
            meta?.let { Text(it, style = Type.meta, color = palette.mut, maxLines = 1) }
        }
    }
}

/** A page in the Inbox design: its own top line, the near-white ground, and a bottom above the navigation bar. */
@Composable
fun InboxPage(
    top: @Composable () -> Unit,
    bottom: (@Composable () -> Unit)? = null,
    background: Color = palette.bg,
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(Modifier.fillMaxSize().background(background)) {
        top()
        Column(Modifier.weight(1f).fillMaxWidth(), content = content)
        if (bottom != null) Box(Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.navigationBars)) { bottom() }
    }
}

/** A row that says something is loading or failed, in a list's place. */
@Composable
fun StateRow(text: String, busy: Boolean = false, error: Boolean = false) {
    Row(Modifier.fillMaxWidth().heightIn(min = 52.dp).padding(vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        if (busy) {
            CircularProgressIndicator(Modifier.size(16.dp), color = palette.mut, strokeWidth = 2.dp)
            Spacer(Modifier.width(10.dp))
        }
        Text(text, style = Type.sub, color = if (error) palette.bad else palette.mut)
    }
}

/** A headline in two tones, "3 things need you. 2 agents working." */
@Composable
fun stateText(main: String, quiet: String? = null): AnnotatedString {
    val mut = palette.mut
    return buildAnnotatedString {
        append(main)
        if (!quiet.isNullOrEmpty()) {
            append(" ")
            withStyle(SpanStyle(color = mut)) { append(quiet) }
        }
    }
}

/** The headline sentence. A quieter second sentence starts its own line when both don't fit on one. */
@Composable
fun StateLine(text: AnnotatedString, modifier: Modifier = Modifier, size: Float = 34f) {
    val style = Type.state.copy(fontSize = Type.state.fontSize * (size / 34f), lineHeight = Type.state.lineHeight * (size / 34f), color = palette.ink)
    val split = text.spanStyles.firstOrNull()?.start
    if (split == null || split <= 0) {
        Text(text, modifier, style = style)
        return
    }
    val measurer = rememberTextMeasurer()
    BoxWithConstraints(modifier) {
        val oneLine = measurer.measure(text, style, maxLines = 1, softWrap = false).size.width <= constraints.maxWidth
        if (oneLine) {
            Text(text, style = style, maxLines = 1)
        } else {
            Column {
                Text(text.subSequence(0, split).toString().trimEnd(), style = style)
                Text(text.subSequence(split, text.length), style = style)
            }
        }
    }
}

/** The pill at the bottom: a tap to type, the mic to speak, and on Home a plus to start. */
@Composable
fun AskBar(placeholder: String, dark: Boolean = false, enabled: Boolean = true, start: (() -> Unit)? = null, type: () -> Unit, speak: () -> Unit) {
    Box(Modifier.fillMaxWidth().background(if (dark) termBg else palette.bg).padding(start = 16.dp, end = 16.dp, top = 10.dp, bottom = 6.dp)) {
        Row(
            Modifier.fillMaxWidth().height(56.dp).shadow(14.dp, CircleShape, ambientColor = Color.Black.copy(alpha = 0.07f), spotColor = Color.Black.copy(alpha = 0.07f))
                .clip(CircleShape).background(palette.sf).border(1.dp, palette.line, CircleShape).padding(start = 6.dp, end = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (start != null) {
                Box(
                    Modifier.size(44.dp).clip(CircleShape).clickable(role = Role.Button, onClick = start)
                        .semantics { contentDescription = "New session" }.testTag("startButton"),
                    Alignment.Center,
                ) { Icon(Icons.Filled.Add, null, Modifier.size(24.dp), tint = palette.ink) }
            }
            Box(
                Modifier.weight(1f).height(44.dp).clickable(enabled = enabled, onClick = type).padding(start = if (start == null) 14.dp else 0.dp)
                    .testTag(if (start == null) "messageBar" else "typeButton"),
                Alignment.CenterStart,
            ) { Text(placeholder, style = Type.field, color = palette.mut, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            MicDisc(
                40.dp, if (start == null) "Speak to this agent" else "Talk to your agents",
                if (start == null) "agentMic" else "micButton", enabled, speak,
            )
        }
    }
}

/** An icon over a word, in the session's action row. */
@Composable
fun ActionButton(icon: ImageVector, label: String, tag: String? = null, enabled: Boolean = true, destructive: Boolean = false, onClick: () -> Unit) {
    Column(
        Modifier.heightIn(min = 48.dp).width(64.dp).clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label }.then(if (tag != null) Modifier.testTag(tag) else Modifier),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(3.dp, Alignment.CenterVertically),
    ) {
        val c = (if (destructive) palette.bad else palette.sub).let { if (enabled) it else it.copy(alpha = 0.4f) }
        Icon(icon, null, Modifier.size(22.dp), tint = c)
        Text(label, style = Type.caption.copy(fontSize = Type.caption.fontSize * 0.92f, fontWeight = FontWeight.Medium), color = c)
    }
}

/** A destructive choice as a sheet: what each does, then its buttons. Cancel is always last. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChoiceSheet(title: String, choices: List<Triple<String, String?, () -> Unit>>, dismiss: () -> Unit) {
    ModalBottomSheet(dismiss, containerColor = palette.sf, shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp)) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 16.dp).windowInsetsPadding(WindowInsets.navigationBars)) {
            Text(title, style = Type.h1, color = palette.ink)
            choices.forEachIndexed { i, (label, detail, _) ->
                if (detail != null) {
                    Column(Modifier.fillMaxWidth().padding(top = if (i == 0) 10.dp else 0.dp)) {
                        if (i > 0) Hairline()
                        Column(Modifier.padding(vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            Text(label, style = Type.title, color = palette.ink)
                            Text(detail, style = Type.sub, color = palette.sub)
                        }
                    }
                }
            }
            Column(Modifier.padding(top = 14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                choices.forEach { (label, _, run) ->
                    BigButton(label, Modifier.fillMaxWidth().testTag("action-$label"), ButtonKind.Outline, destructive = true) {
                        dismiss()
                        run()
                    }
                }
                BigButton("Cancel", Modifier.fillMaxWidth(), ButtonKind.Plain, color = palette.sub, onClick = dismiss)
            }
        }
    }
}

/** Chips in a row that scrolls sideways when they don't fit. */
@Composable
fun ChipRow(content: @Composable RowScope.() -> Unit) {
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp), content = content)
}

/** A thin bar that empties as time runs out. */
@Composable
fun CountdownBar(fraction: Float) {
    Box(Modifier.fillMaxWidth().height(3.dp).clip(CircleShape).background(palette.fill)) {
        Box(Modifier.fillMaxWidth(fraction.coerceIn(0f, 1f)).height(3.dp).clip(CircleShape).background(palette.warn))
    }
}

