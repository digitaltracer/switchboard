package com.digitaltracer.switchboard

import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Build
import androidx.compose.material.icons.outlined.Cancel
import androidx.compose.material.icons.outlined.Checklist
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.Extension
import androidx.compose.material.icons.outlined.Forum
import androidx.compose.material.icons.outlined.Group
import androidx.compose.material.icons.outlined.Lightbulb
import androidx.compose.material.icons.outlined.Public
import androidx.compose.material.icons.outlined.QuestionAnswer
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material3.CircularProgressIndicator
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
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitaltracer.switchboard.api.models.Transcript
import com.digitaltracer.switchboard.api.models.TranscriptItem
import com.digitaltracer.switchboard.core.Conversation
import com.digitaltracer.switchboard.core.Hub
import com.digitaltracer.switchboard.core.HubError
import kotlin.math.min
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Whose conversation: a Switchboard session's, or an agent started in Herdr. */
sealed interface ConversationSource {
    data class Session(val id: String) : ConversationSource
    data class Agent(val machine: String, val name: String) : ConversationSource
}

/** One agent's conversation, and its terminal, followed while a screen shows them. */
class ConversationModel(val source: ConversationSource) {
    var conversation by mutableStateOf(Conversation())
        private set
    /** Why there's no conversation, e.g. no log yet or an older hub; the terminal stands in. */
    var unavailable by mutableStateOf<String?>(null)
        private set
    var terminal by mutableStateOf("")
        private set
    /** False until the screen has been read once. */
    var terminalLoaded by mutableStateOf(false)
        private set
    var terminalError by mutableStateOf<String?>(null)
        private set
    var error by mutableStateOf<String?>(null)
        private set
    var loadingEarlier by mutableStateOf(false)
        private set
    /** The terminal is read only while it's on screen or standing in. */
    var wantsTerminal = false

    /** Polls until cancelled. */
    suspend fun follow(model: AppModel) {
        while (true) {
            model.hub?.let { hub ->
                refresh(hub)
                if (wantsTerminal || unavailable != null) loadTerminal(hub)
            }
            delay(3_000)
        }
    }

    suspend fun refresh(hub: Hub) {
        try {
            conversation = if (conversation.isLoaded) {
                val after = conversation.end
                conversation.append(page(hub, after, null), askedAfter = after)
            } else {
                Conversation.of(page(hub, null, null))
            }
            unavailable = null
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: HubError) {
            when (e.code) {
                // A hub older than this app has no conversations to give.
                "http_404" -> unavailable = "the hub needs updating"
                "bad_request", "not_found" -> unavailable = e.message
                else -> error = e.message
            }
        } catch (e: Exception) {
            error = e.message
        }
    }

    suspend fun loadEarlier(hub: Hub) {
        val start = conversation.start ?: return
        if (start <= 0 || loadingEarlier) return
        loadingEarlier = true
        try {
            conversation = conversation.prepend(page(hub, null, start))
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message
        } finally {
            loadingEarlier = false
        }
    }

    /** Only the screen: reading further back would scroll the agent's terminal on the Mac. */
    suspend fun loadTerminal(hub: Hub) {
        try {
            terminal = when (val s = source) {
                is ConversationSource.Session -> hub.output(s.id, visible = true).text
                is ConversationSource.Agent -> hub.agentOutput(s.machine, s.name, visible = true).text
            }
            terminalLoaded = true
            terminalError = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            terminalError = e.message
        }
    }

    private suspend fun page(hub: Hub, after: Long?, before: Long?): Transcript = when (val s = source) {
        is ConversationSource.Session -> hub.transcript(s.id, after, before)
        is ConversationSource.Agent -> hub.agentTranscript(s.machine, s.name, after, before)
    }
}

@Composable
fun rememberConversation(source: ConversationSource): ConversationModel = remember(source) { ConversationModel(source) }

/** The whole conversation, with the terminal a tab away. */
@Composable
fun ConversationScreen(model: AppModel, source: ConversationSource, title: String) {
    val convo = rememberConversation(source)
    var terminalTab by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(source, model.wakes) { convo.follow(model) }

    Page(model.backLabel, model::pop) {
        Column(Modifier.fillMaxSize()) {
            Text(title, Modifier.fillMaxWidth().padding(horizontal = 16.dp), style = Type.headline, color = palette.label, maxLines = 1, textAlign = TextAlign.Center)
            val why = convo.unavailable
            if (why != null) {
                // Nothing to switch between: the terminal is all there is.
                Text(
                    "No conversation to show ($why), so here's the agent's screen.",
                    Modifier.padding(horizontal = 16.dp, vertical = 10.dp), style = Type.footnote, color = palette.secondary,
                )
                Screen(convo)
                return@Column
            }
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp).card(9.dp, palette.card).padding(2.dp),
            ) {
                listOf(false to "Conversation", true to "Terminal").forEach { (isTerminal, label) ->
                    val selected = terminalTab == isTerminal
                    Text(
                        label,
                        Modifier.weight(1f)
                            .card(7.dp, if (selected) palette.background else Color.Transparent)
                            .clickable {
                                terminalTab = isTerminal
                                convo.wantsTerminal = isTerminal
                                // Shown at once, not at the next refresh.
                                if (isTerminal) scope.launch { model.hub?.let { convo.loadTerminal(it) } }
                            }
                            .padding(vertical = 6.dp)
                            .testTag(label),
                        style = Type.subheadline, fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal,
                        color = palette.label, textAlign = TextAlign.Center,
                    )
                }
            }
            if (terminalTab) {
                Screen(convo)
            } else {
                Feed(model, convo, source) { scope.launch { model.hub?.let { convo.loadEarlier(it) } } }
            }
        }
    }
}

/** The agent's screen once read; until then a loading or error message. */
@Composable
private fun Screen(convo: ConversationModel) {
    val error = convo.terminalError
    when {
        convo.terminalLoaded -> TerminalView(convo.terminal)
        error != null -> Waiting(Icons.Outlined.ErrorOutline, "Couldn't read the screen: $error", busy = false)
        else -> Waiting(Icons.Outlined.Terminal, "Loading the screen…", busy = true)
    }
}

/** A centred symbol and line, with a spinner while something's on its way. */
@Composable
fun Waiting(icon: ImageVector, text: String, busy: Boolean, modifier: Modifier = Modifier.fillMaxSize()) {
    Column(modifier.padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
        if (busy) {
            CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
        } else {
            Icon(icon, null, Modifier.size(28.dp), tint = palette.secondary)
        }
        Text(text, Modifier.padding(top = 12.dp), style = Type.subheadline, color = palette.secondary, textAlign = TextAlign.Center)
    }
}

/** A message, a reply and a tool call drawn as grey shapes that pulse while the conversation loads. */
@Composable
fun ConversationSkeleton(rows: Int = 3) {
    val pulse = rememberInfiniteTransition(label = "skeleton")
    val alpha by pulse.animateFloat(1f, 0.45f, infiniteRepeatable(tween(900), RepeatMode.Reverse), label = "alpha")
    val fill = palette.separator
    Column(
        Modifier.fillMaxWidth().alpha(alpha).semantics { contentDescription = "Loading the conversation" },
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        repeat(rows) {
            Box(Modifier.fillMaxWidth(), Alignment.CenterEnd) { Box(Modifier.width(190.dp).height(38.dp).card(18.dp, fill)) }
            Column(verticalArrangement = Arrangement.spacedBy(7.dp)) {
                Box(Modifier.fillMaxWidth().height(12.dp).card(6.dp, fill))
                Box(Modifier.fillMaxWidth().padding(end = 40.dp).height(12.dp).card(6.dp, fill))
                Box(Modifier.width(150.dp).height(12.dp).card(6.dp, fill))
            }
            Box(Modifier.fillMaxWidth().height(38.dp).card(12.dp, fill))
        }
    }
}

@Composable
private fun Feed(model: AppModel, convo: ConversationModel, source: ConversationSource, loadEarlier: () -> Unit) {
    val items = convo.conversation.items
    val list = rememberLazyListState()
    // Start at the latest, and keep up with new items unless scrolled back.
    LaunchedEffect(items.size) {
        val last = list.layoutInfo.totalItemsCount - 1
        val atEnd = list.layoutInfo.visibleItemsInfo.lastOrNull()?.index?.let { it >= last - 2 } ?: true
        if (atEnd && last > 0) list.scrollToItem(last)
    }
    LazyColumn(
        Modifier.fillMaxSize().testTag("conversation"),
        state = list,
        contentPadding = androidx.compose.foundation.layout.PaddingValues(start = 16.dp, end = 16.dp, bottom = 24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (convo.conversation.hasEarlier) {
            item {
                Box(Modifier.fillMaxWidth(), Alignment.Center) {
                    if (convo.loadingEarlier) {
                        CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    } else {
                        Text("Show earlier messages", Modifier.clickable(onClick = loadEarlier).padding(8.dp), style = Type.body, color = palette.accent)
                    }
                }
            }
        }
        val error = convo.error
        if (!convo.conversation.isLoaded) {
            item {
                if (error != null) {
                    Waiting(Icons.Outlined.ErrorOutline, "Couldn't load the conversation: $error", busy = false, Modifier.fillMaxWidth().padding(top = 40.dp))
                } else {
                    ConversationSkeleton()
                }
            }
        } else if (items.isEmpty()) {
            item { Waiting(Icons.Outlined.Forum, "Nothing said yet.", busy = false, Modifier.fillMaxWidth().padding(top = 40.dp)) }
        }
        items(items, key = { it.id }) { ItemRow(it) }
        if (convo.conversation.isLoaded) {
            status(model, source)?.let { s -> item { StatusLine(s) } }
            error?.let { e -> item { Text(e, style = Type.footnote, color = palette.needs) } }
        }
    }
}

private fun status(model: AppModel, source: ConversationSource): SessionStatus? = when (source) {
    is ConversationSource.Session -> model.sessions.firstOrNull { it.id == source.id }?.let(SessionStatus::of)
    is ConversationSource.Agent ->
        model.agents.firstOrNull { it.machineId == source.machine && it.name == source.name }?.let { SessionStatus.agent(it.agentStatus) }
}

/** The latest few items, for a session or agent page. */
@Composable
fun ConversationPreview(convo: ConversationModel, count: Int = 6) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        when {
            !convo.conversation.isLoaded -> ConversationSkeleton(rows = 2)
            convo.conversation.items.isEmpty() -> Text("Nothing said yet.", style = Type.subheadline, color = palette.secondary)
        }
        convo.conversation.items.takeLast(count).forEach { ItemRow(it) }
    }
}

/** One entry: your message, Claude's reply, a tool call or a note. */
@Composable
fun ItemRow(item: TranscriptItem) {
    when (item.kind) {
        "user" -> Box(Modifier.fillMaxWidth().padding(start = 40.dp), Alignment.CenterEnd) {
            SelectionContainer {
                Text(
                    item.text.orEmpty(),
                    Modifier.card(18.dp, palette.accent).padding(horizontal = 13.dp, vertical = 9.dp),
                    style = Type.body, color = Color.White,
                )
            }
        }
        "assistant" -> MarkdownText(item.text.orEmpty())
        "tool" -> ToolRow(item)
        else -> Text(item.text.orEmpty(), Modifier.fillMaxWidth(), style = Type.caption, color = palette.secondary, textAlign = TextAlign.Center)
    }
}

@Composable
private fun ToolRow(item: TranscriptItem) {
    var expanded by remember { mutableStateOf(false) }
    Column(
        Modifier.fillMaxWidth().card(12.dp, palette.card).clickable { expanded = !expanded }
            .padding(horizontal = 12.dp, vertical = 9.dp).animateContentSize(),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(toolIcon(item.tool.orEmpty()), null, Modifier.padding(top = 2.dp).size(16.dp), tint = palette.secondary)
            Text(
                item.text ?: item.tool ?: "Tool", Modifier.weight(1f),
                style = Type.subheadline, color = palette.label, maxLines = if (expanded) Int.MAX_VALUE else 2,
            )
            when {
                !item.done -> CircularProgressIndicator(Modifier.padding(top = 2.dp).size(14.dp), strokeWidth = 1.5.dp)
                item.failed -> Icon(Icons.Outlined.Cancel, "Failed", Modifier.padding(top = 2.dp).size(16.dp), tint = palette.red)
            }
        }
        if (expanded) {
            item.input?.takeIf { it.isNotEmpty() }?.let { CodeBox("Input", it) }
            item.output?.let { CodeBox(if (item.failed) "Failed" else "Result", it.ifEmpty { "(nothing)" }) }
        }
    }
}

private fun toolIcon(tool: String): ImageVector = when (tool) {
    "Bash" -> Icons.Outlined.Terminal
    "Read" -> Icons.Outlined.Description
    "Edit", "MultiEdit", "Write", "NotebookEdit" -> Icons.Outlined.Edit
    "Grep", "Glob" -> Icons.Outlined.Search
    "WebFetch", "WebSearch" -> Icons.Outlined.Public
    "Task", "Agent" -> Icons.Outlined.Group
    "TodoWrite" -> Icons.Outlined.Checklist
    "AskUserQuestion" -> Icons.Outlined.QuestionAnswer
    "ExitPlanMode" -> Icons.Outlined.Lightbulb
    else -> if (tool.startsWith("mcp__")) Icons.Outlined.Extension else Icons.Outlined.Build
}

/** A tool's input or result: monospaced, scrolling sideways instead of wrapping. */
@Composable
fun CodeBox(title: String, text: String) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        if (title.isNotEmpty()) Text(title.uppercase(), style = Type.caption, fontWeight = FontWeight.SemiBold, color = palette.secondary)
        SelectionContainer(Modifier.fillMaxWidth().card(10.dp, codeBackground).horizontalScroll(rememberScrollState())) {
            Text(text, Modifier.padding(10.dp), color = Color(0xFFE6E6E6), fontFamily = FontFamily.Monospace, fontSize = 12.sp, softWrap = false)
        }
    }
}

/** Where the agent is now, under the last message. */
@Composable
private fun StatusLine(status: SessionStatus) {
    Row(Modifier.fillMaxWidth().padding(top = 4.dp), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
        if (status.kind == SessionStatus.Kind.Working) {
            CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 1.5.dp, color = status.color)
        } else {
            Icon(status.symbol, null, Modifier.size(16.dp), tint = status.color)
        }
        Text(status.text, Modifier.padding(start = 8.dp), style = Type.footnote, fontWeight = FontWeight.Medium, color = status.color)
    }
}

/**
 * The agent's screen like a photo of it: fitted to the phone's width at first, pinched to zoom,
 * double-tapped between fitted and full size. Text is laid out at full size, so it stays sharp.
 */
@Composable
fun TerminalView(text: String) {
    val measurer = rememberTextMeasurer()
    val style = TextStyle(color = Color(0xFFE6E6E6), fontFamily = FontFamily.Monospace, fontSize = 12.sp)
    if (text.isBlank()) {
        Box(Modifier.fillMaxSize().background(codeBackground).testTag("terminal"), Alignment.Center) {
            Text("The screen is blank.", style = Type.subheadline, color = Color(0xFF999999))
        }
        return
    }
    val layout = remember(text) { measurer.measure(text, style, softWrap = false) }
    BoxWithConstraints(Modifier.fillMaxSize().background(codeBackground).clipToBounds().testTag("terminal")) {
        val pad = with(LocalDensity.current) { 12.dp.toPx() }
        val view = Size(constraints.maxWidth.toFloat(), constraints.maxHeight.toFloat())
        val content = Size(layout.size.width + 2 * pad, layout.size.height + 2 * pad)
        val fit = min(1f, view.width / content.width)
        var scale by remember { mutableStateOf<Float?>(null) }
        var offset by remember { mutableStateOf(Offset.Zero) }
        val s = scale ?: fit
        // Keeps the content on screen; when it's smaller than the screen it sits top left.
        fun clamp(o: Offset, at: Float) = Offset(
            o.x.coerceIn(min(0f, view.width - content.width * at), 0f),
            o.y.coerceIn(min(0f, view.height - content.height * at), 0f),
        )
        val bottom = view.height - content.height * s
        // Starts at the bottom, and stays there as new screens arrive unless scrolled up.
        LaunchedEffect(layout) {
            if (scale == null || offset.y <= bottom + 48) offset = clamp(Offset(offset.x, bottom), s)
        }
        Canvas(
            Modifier.fillMaxSize()
                .pointerInput(fit) {
                    detectTransformGestures { centroid, pan, zoom, _ ->
                        val from = scale ?: fit
                        val to = (from * zoom).coerceIn(fit, 1.5f)
                        // Zooms about the fingers: the point under them stays put.
                        val next = (offset - centroid) * (to / from) + centroid + pan
                        scale = to
                        offset = clamp(next, to)
                    }
                }
                .pointerInput(fit) {
                    detectTapGestures(onDoubleTap = { at ->
                        val from = scale ?: fit
                        val to = if (from > fit + 0.01f) fit else 1f
                        scale = to
                        offset = clamp((offset - at) * (to / from) + at, to)
                    })
                },
        ) {
            withTransform({
                translate(offset.x, offset.y)
                scale(s, s, pivot = Offset.Zero)
            }) {
                drawText(layout, topLeft = Offset(pad, pad))
            }
        }
    }
}

/**
 * Claude's Markdown, enough of it for a phone: paragraphs with bold, italics, inline code and
 * links, headings, lists, code blocks and tables. Code and tables scroll sideways.
 */
@Composable
fun MarkdownText(text: String) {
    SelectionContainer {
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Markdown.blocks(text).forEach { b ->
                when (b) {
                    is Markdown.Block.Text -> Text(Markdown.inline(b.text), style = Type.body, color = palette.label)
                    is Markdown.Block.Heading -> Text(Markdown.inline(b.text), style = Type.headline, color = palette.label)
                    is Markdown.Block.Code -> CodeBox("", b.text)
                    is Markdown.Block.Table -> Box(Modifier.horizontalScroll(rememberScrollState())) {
                        Text(b.text, color = palette.label, fontFamily = FontFamily.Monospace, fontSize = 12.sp, softWrap = false)
                    }
                }
            }
        }
    }
}

object Markdown {
    sealed interface Block {
        data class Text(val text: String) : Block
        data class Heading(val text: String) : Block
        data class Code(val text: String) : Block
        data class Table(val text: String) : Block
    }

    fun blocks(text: String): List<Block> {
        val out = mutableListOf<Block>()
        val para = mutableListOf<String>()
        val table = mutableListOf<String>()
        var code: MutableList<String>? = null
        fun flushPara() {
            if (para.isNotEmpty()) out += Block.Text(para.joinToString("\n")).also { para.clear() }
        }
        fun flushTable() {
            if (table.isNotEmpty()) out += Block.Table(table.joinToString("\n")).also { table.clear() }
        }
        for (line in text.split("\n")) {
            val t = line.trim()
            val inCode = code
            when {
                t.startsWith("```") -> if (inCode != null) {
                    out += Block.Code(inCode.joinToString("\n"))
                    code = null
                } else {
                    flushPara(); flushTable(); code = mutableListOf()
                }
                inCode != null -> inCode += line
                t.startsWith("|") -> { flushPara(); table += t }
                t.startsWith("#") -> { flushPara(); flushTable(); out += Block.Heading(t.trimStart('#').trim()) }
                t.isEmpty() -> { flushPara(); flushTable() }
                else -> { flushTable(); para += bullet(line) }
            }
        }
        code?.let { out += Block.Code(it.joinToString("\n")) }
        flushPara()
        flushTable()
        return out
    }

    /** `- item` and `* item` as bullets; numbered items stay as written. */
    private fun bullet(line: String): String {
        val indent = line.takeWhile { it == ' ' }
        val rest = line.drop(indent.length)
        return if (rest.startsWith("- ") || rest.startsWith("* ")) "$indent• ${rest.drop(2)}" else line
    }

    /** `**bold**`, `*italic*`, `` `code` `` and `[text](url)`, shown as text. */
    fun inline(s: String): AnnotatedString = buildAnnotatedString {
        var i = 0
        while (i < s.length) {
            val rest = s.substring(i)
            fun span(open: String, close: String = open): String? =
                if (rest.startsWith(open)) rest.indexOf(close, open.length).takeIf { it > open.length }?.let { rest.substring(open.length, it) } else null
            val bold = span("**")
            val code = if (bold == null) span("`") else null
            val italic = if (bold == null && code == null) span("*")?.takeIf { !it.startsWith(" ") } else null
            val link = if (rest.startsWith("[")) Regex("""^\[([^\]]+)]\(([^)]+)\)""").find(rest) else null
            when {
                bold != null -> { withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { append(bold) }; i += bold.length + 4 }
                code != null -> { withStyle(SpanStyle(fontFamily = FontFamily.Monospace, fontSize = 15.sp)) { append(code) }; i += code.length + 2 }
                italic != null -> { withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { append(italic) }; i += italic.length + 2 }
                link != null -> { withStyle(SpanStyle(color = Color(0xFF0A84FF))) { append(link.groupValues[1]) }; i += link.value.length }
                else -> { append(s[i]); i++ }
            }
        }
    }
}
