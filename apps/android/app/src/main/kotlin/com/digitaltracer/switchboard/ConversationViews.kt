package com.digitaltracer.switchboard

import com.digitaltracer.switchboard.core.Markdown
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.layout.layout
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.ui.unit.Dp
import androidx.compose.foundation.layout.Spacer
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
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowUp
import androidx.compose.material.icons.filled.Mic
import androidx.compose.foundation.layout.heightIn
import com.digitaltracer.switchboard.core.Turns
import com.digitaltracer.switchboard.core.agentTarget
import com.digitaltracer.switchboard.core.sessionTarget
import com.digitaltracer.switchboard.core.MarkdownTable
import androidx.compose.material3.HorizontalDivider
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
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitaltracer.switchboard.api.models.Transcript
import com.digitaltracer.switchboard.api.models.TranscriptItem
import com.digitaltracer.switchboard.core.Ansi
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
    /** The same screen with its colours, when the hub sends them. */
    var terminalAnsi by mutableStateOf<String?>(null)
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
            val out = when (val s = source) {
                is ConversationSource.Session -> hub.output(s.id, visible = true, ansi = true)
                is ConversationSource.Agent -> hub.agentOutput(s.machine, s.name, visible = true, ansi = true)
            }
            terminal = out.text
            terminalAnsi = out.ansi
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

/** The whole conversation, with the agent's screen a tab away, and a message bar under both. */
@Composable
fun ConversationScreen(model: AppModel, voice: Voice, listen: Listen, source: ConversationSource, title: String, screen: Boolean) {
    val convo = rememberConversation(source)
    var terminalTab by remember { mutableStateOf(screen) }
    val scope = rememberCoroutineScope()
    LaunchedEffect(source, model.wakes) {
        convo.wantsTerminal = terminalTab
        convo.follow(model)
    }
    val target = when (source) {
        is ConversationSource.Session -> model.sessions.firstOrNull { it.id == source.id }?.let(::sessionTarget)
        is ConversationSource.Agent -> agentTarget(source.machine, source.name, title)
    }

    Page(model.backLabel, model::pop, bottomBar = { MessageBar(target != null, { voice.type(target) }) { listen(target, false) } }) {
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
                listOf(false to "Conversation", true to "Screen").forEach { (isTerminal, label) ->
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
        convo.terminalLoaded -> TerminalView(convo.terminal, convo.terminalAnsi)
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
        val speaker = speaker(model, source)
        items(Turns.entries(items), key = { it.id }) { EntryRow(it, speaker) }
        if (convo.conversation.isLoaded) {
            status(model, source)?.let { s -> item { StatusLine(s) } }
            error?.let { e -> item { Text(e, style = Type.footnote, color = palette.needs) } }
        }
    }
}

/** Who replies: `Claude` or `Codex`. */
private fun speaker(model: AppModel, source: ConversationSource): String = when (source) {
    is ConversationSource.Session -> model.sessions.firstOrNull { it.id == source.id }?.agent?.capitalized()
    is ConversationSource.Agent -> model.agents.firstOrNull { it.machineId == source.machine && it.name == source.name }?.kind?.capitalized()
} ?: "Agent"

/** Opens the review sheet, where `/` lists the agent's skills; the mic talks instead. */
@Composable
private fun MessageBar(enabled: Boolean, type: () -> Unit, talk: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(
            Modifier.weight(1f).heightIn(min = 44.dp).card(22.dp, palette.card).clickable(enabled = enabled, onClick = type)
                .padding(horizontal = 16.dp).testTag("messageBar"),
            Alignment.CenterStart,
        ) { Text("Message, or / for skills", style = Type.body, color = palette.secondary) }
        Box(
            Modifier.size(44.dp).card(22.dp, palette.accent).clickable(enabled = enabled, onClick = talk)
                .semantics { contentDescription = "Talk" },
            Alignment.Center,
        ) { Icon(Icons.Filled.Mic, null, Modifier.size(22.dp), tint = Color.White) }
    }
}

private fun status(model: AppModel, source: ConversationSource): SessionStatus? = when (source) {
    is ConversationSource.Session -> model.sessions.firstOrNull { it.id == source.id }?.let(SessionStatus::of)
    is ConversationSource.Agent ->
        model.agents.firstOrNull { it.machineId == source.machine && it.name == source.name }?.let { SessionStatus.agent(it.agentStatus) }
}

/** What the agent last said and what its work since your last message came to, for a session or agent page. */
@Composable
fun SummaryCard(convo: ConversationModel, open: () -> Unit) {
    val latest = Turns.latest(convo.conversation.items)
    Column(
        Modifier.fillMaxWidth().card(16.dp, palette.card).padding(start = 14.dp, end = 14.dp, top = 14.dp, bottom = 4.dp).testTag("latest"),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("LATEST REPLY", Modifier.weight(1f), style = Type.caption, fontWeight = FontWeight.SemiBold, color = palette.secondary)
            latest.at?.let(Turns::time)?.let { Text(it, style = Type.caption, color = palette.secondary) }
        }
        val why = convo.unavailable
        val reply = latest.reply?.text
        when {
            why != null -> Text("No conversation to show ($why).", style = Type.footnote, color = palette.secondary)
            !convo.conversation.isLoaded -> ConversationSkeleton(rows = 1)
            reply != null -> Clamped(286.dp) { MarkdownText(reply) }
            else -> Text("Nothing said yet.", style = Type.subheadline, color = palette.secondary)
        }
        Row(Modifier.fillMaxWidth().heightIn(min = 44.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.weight(1f)) { if (latest.tags.isNotEmpty()) TagRow(latest.tags) }
            if (reply != null) {
                Text(
                    "Read all", Modifier.clickable(onClick = open).padding(start = 12.dp, top = 10.dp, bottom = 10.dp),
                    style = Type.subheadline, fontWeight = FontWeight.SemiBold, color = palette.accent,
                )
            }
        }
    }
}

/** At most [max] tall; cut-off content fades into the card. */
@Composable
private fun Clamped(max: Dp, content: @Composable () -> Unit) {
    var cut by remember { mutableStateOf(false) }
    Box(Modifier.fillMaxWidth().clipToBounds()) {
        Box(
            Modifier.layout { measurable, constraints ->
                val limit = max.roundToPx()
                val p = measurable.measure(constraints.copy(maxHeight = Constraints.Infinity))
                cut = p.height > limit
                layout(p.width, min(p.height, limit)) { p.place(0, 0) }
            },
        ) { content() }
        if (cut) {
            Box(
                Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(56.dp)
                    .background(Brush.verticalGradient(listOf(palette.card.copy(alpha = 0f), palette.card))),
            )
        }
    }
}

/** The two ways into an agent: its conversation and its screen. */
@Composable
fun ConversationLinks(model: AppModel, source: ConversationSource, title: String, hasConversation: Boolean) {
    Column(Modifier.fillMaxWidth().card(16.dp, palette.card)) {
        if (hasConversation) {
            ListRow(Icons.Outlined.Forum, palette.accent, "Conversation", null, "", true, tag = "openConversation") {
                model.path.add(Route.Conversation(source, title))
            }
        }
        ListRow(Icons.Outlined.Terminal, palette.accent, "Screen", null, "", !hasConversation, tag = "openScreen") {
            model.path.add(Route.Conversation(source, title, screen = true))
        }
    }
}

/** Your message, the agent's reply, its folded work or a note. */
@Composable
private fun EntryRow(entry: Turns.Entry, speaker: String) {
    when (entry) {
        is Turns.Entry.User -> ItemRow(entry.item)
        is Turns.Entry.Note -> ItemRow(entry.item)
        is Turns.Entry.Reply -> Column(Modifier.padding(top = 4.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(
                listOfNotNull(speaker, entry.item.at?.let(Turns::time)).joinToString(" · "),
                style = Type.caption, fontWeight = FontWeight.SemiBold, color = palette.secondary,
            )
            MarkdownText(entry.item.text.orEmpty())
        }
        is Turns.Entry.Progress -> MarkdownText(entry.item.text.orEmpty(), ReplyTone.Progress)
        is Turns.Entry.Work -> WorkRow(entry)
    }
}

/** The steps between two messages in one quiet line, "Worked 6m · Tests passed"; tapped, the steps. */
@Composable
private fun WorkRow(work: Turns.Entry.Work) {
    var expanded by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().animateContentSize()) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 36.dp).clickable { expanded = !expanded }.testTag("work")
                .semantics { contentDescription = if (expanded) "Hide the steps" else "Show the ${work.items.size} steps" },
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(if (expanded) Icons.Filled.KeyboardArrowDown else Icons.AutoMirrored.Filled.KeyboardArrowRight, null, Modifier.size(16.dp), tint = palette.secondary)
            if (work.running) CircularProgressIndicator(Modifier.size(11.dp), strokeWidth = 1.5.dp, color = palette.secondary)
            Text(workLine(work), style = Type.footnote, color = palette.secondary, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (expanded) {
            Column(Modifier.padding(start = 22.dp, top = 2.dp, bottom = 6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                work.items.forEach { ToolRow(it) }
            }
        }
    }
}

/** "Worked 54s · 1 file +48 −6 · Tests passed", the results in their colours. */
@Composable
private fun workLine(work: Turns.Entry.Work): AnnotatedString {
    val p = palette
    return buildAnnotatedString {
        append(if (work.running) "Working" else work.seconds?.let { "Worked ${Turns.duration(it)}" } ?: "Worked")
        work.tags.forEach { tag ->
            append(" · ")
            val color = when (tag.tone) {
                Turns.Tone.Good -> p.done
                Turns.Tone.Bad -> p.red
                Turns.Tone.Plain -> Color.Unspecified
            }
            withStyle(SpanStyle(color = color)) { append(tag.text) }
            tag.added?.let { withStyle(SpanStyle(color = p.done)) { append(" +$it") } }
            tag.removed?.takeIf { it > 0 }?.let { withStyle(SpanStyle(color = p.red)) { append(" −$it") } }
        }
    }
}

/** "1 file +48 −6", "Tests passed", "PR #49": as many as fit on one line. */
@Composable
private fun TagRow(tags: List<Turns.Tag>) {
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        tags.forEach { TagChip(it) }
    }
}

@Composable
private fun TagChip(tag: Turns.Tag) {
    val (fg, bg) = when (tag.tone) {
        Turns.Tone.Good -> palette.done to palette.done.copy(alpha = 0.14f)
        Turns.Tone.Bad -> palette.red to palette.red.copy(alpha = 0.12f)
        Turns.Tone.Plain -> palette.label to palette.fill
    }
    Text(
        buildAnnotatedString {
            append(tag.text)
            tag.added?.let { withStyle(SpanStyle(color = palette.done)) { append(" +$it") } }
            tag.removed?.takeIf { it > 0 }?.let { withStyle(SpanStyle(color = palette.red)) { append(" −$it") } }
        },
        Modifier.card(10.dp, bg).padding(horizontal = 8.dp, vertical = 3.dp),
        style = Type.caption, color = fg, maxLines = 1,
    )
}

/** One entry: your message, Claude's reply, a tool call or a note. */
@Composable
fun ItemRow(item: TranscriptItem) {
    when (item.kind) {
        "user" -> Box(Modifier.fillMaxWidth().padding(start = 56.dp), Alignment.CenterEnd) {
            SelectionContainer {
                Text(
                    item.text.orEmpty(),
                    Modifier.clip(RoundedCornerShape(18.dp, 18.dp, 4.dp, 18.dp)).background(palette.accent).padding(horizontal = 12.dp, vertical = 8.dp),
                    style = Type.message, color = Color.White,
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
fun TerminalView(text: String, ansi: String? = null) {
    val measurer = rememberTextMeasurer()
    val style = TextStyle(color = Color(0xFFE6E6E6), fontFamily = FontFamily.Monospace, fontSize = 12.sp)
    if (text.isBlank()) {
        Box(Modifier.fillMaxSize().background(codeBackground).testTag("terminal"), Alignment.Center) {
            Text("The screen is blank.", style = Type.subheadline, color = Color(0xFF999999))
        }
        return
    }
    val layout = remember(text, ansi) {
        if (ansi != null) measurer.measure(coloured(ansi), style, softWrap = false) else measurer.measure(text, style, softWrap = false)
    }
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

/** The screen's own colours on the dark background; dim text is drawn at half strength. */
private fun coloured(ansi: String): AnnotatedString = buildAnnotatedString {
    fun color(c: Ansi.Color) = Color(c.r, c.g, c.b)
    for (run in Ansi.parse(ansi)) {
        var fg = run.style.fg?.let(::color) ?: Color(0xFFE6E6E6)
        var bg = run.style.bg?.let(::color)
        if (run.style.inverse) {
            val swapped = bg ?: codeBackground
            bg = fg
            fg = swapped
        }
        if (run.style.dim) fg = fg.copy(alpha = 0.55f)
        withStyle(
            SpanStyle(
                color = fg,
                background = bg ?: Color.Unspecified,
                fontWeight = if (run.style.bold) FontWeight.Bold else null,
                fontStyle = if (run.style.italic) FontStyle.Italic else null,
                textDecoration = if (run.style.underline) TextDecoration.Underline else null,
            ),
        ) { append(run.text) }
    }
}

/** The agent's answer in reading type, or a note on its way there in smaller, quieter type. */
enum class ReplyTone { Answer, Progress }

/**
 * Claude's Markdown, enough of it for a phone: paragraphs with bold, italics, inline code and
 * links, headings, nested lists, code blocks and tables. Code and wide tables scroll sideways.
 */
@Composable
fun MarkdownText(text: String, tone: ReplyTone = ReplyTone.Answer) {
    val answer = tone == ReplyTone.Answer
    val style = if (answer) Type.reply else Type.progress
    val color = if (answer) palette.body else palette.secondary
    // Bold stands out in an answer; in a note it only gets heavier.
    val strong = if (answer) palette.label else Color.Unspecified
    val blocks = remember(text) { Markdown.blocks(text) }
    SelectionContainer {
        Column(Modifier.fillMaxWidth()) {
            blocks.forEachIndexed { i, b ->
                if (i > 0) Spacer(Modifier.height(if (b is Markdown.Block.Heading) 14.dp else 8.dp))
                when (b) {
                    is Markdown.Block.Paragraph -> Text(inline(b.text, strong), style = style, color = color)
                    is Markdown.Block.Heading ->
                        if (answer && b.level <= 2) {
                            Text(inline(b.text), style = Type.replyHeading, color = palette.label)
                        } else {
                            Text(inline(b.text.uppercase()), style = Type.replySection, color = palette.secondary)
                        }
                    is Markdown.Block.List -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        b.items.forEach { item ->
                            Row(Modifier.padding(start = (18 * item.level).dp)) {
                                val bullet = item.marker == "•"
                                Text(
                                    if (bullet && item.level > 0) "◦" else item.marker,
                                    Modifier.width(if (bullet) 14.dp else 22.dp), style = style, color = palette.secondary,
                                )
                                Text(inline(item.text, strong), style = style, color = if (item.level > 0 && answer) palette.bodyDim else color)
                            }
                        }
                    }
                    is Markdown.Block.Code -> CodeBox("", b.text)
                    is Markdown.Block.Table -> MarkdownTable.parse(b.text)?.let { TableView(it) }
                        ?: Text(b.text, style = style, color = color)
                    Markdown.Block.Rule -> HorizontalDivider(Modifier.padding(vertical = 4.dp), thickness = 0.5.dp, color = palette.separator)
                }
            }
        }
    }
}

/** A Markdown table as a grid: each column's width in proportion to its longest cell, long cells wrapping. More than four columns scroll sideways. */
@Composable
private fun TableView(table: MarkdownTable) {
    val wide = table.columns > 4
    Box(if (wide) Modifier.horizontalScroll(rememberScrollState()) else Modifier.fillMaxWidth()) {
        Column(if (wide) Modifier.width((table.weights.sum() * 8 + 24).dp) else Modifier.fillMaxWidth()) {
            if (table.header.isNotEmpty()) TableRow(table.header, table.weights, header = true)
            table.rows.forEachIndexed { i, row ->
                if (i > 0 || table.header.isNotEmpty()) HorizontalDivider(thickness = 0.5.dp, color = palette.separator)
                TableRow(row, table.weights, header = false)
            }
        }
    }
}

@Composable
private fun TableRow(cells: List<String>, weights: List<Float>, header: Boolean) {
    Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        cells.forEachIndexed { i, cell ->
            Text(
                inline(cell), Modifier.weight(weights[i]),
                style = if (header) Type.footnote else Type.subheadline,
                fontWeight = if (header) FontWeight.SemiBold else null,
                color = if (header) palette.secondary else palette.label,
            )
        }
    }
}

/** `**bold**`, `*italic*`, `` `code` `` and `[text](url)`: bold brighter, code in a quiet tint, links in the accent. */
@Composable
private fun inline(s: String, strong: Color = palette.label): AnnotatedString {
    val p = palette
    return remember(s, p, strong) {
        buildAnnotatedString {
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
                    bold != null -> { withStyle(SpanStyle(fontWeight = FontWeight.SemiBold, color = strong)) { append(bold) }; i += bold.length + 4 }
                    // Thin spaces give the tint a little room either side.
                    code != null -> {
                        withStyle(SpanStyle(fontFamily = FontFamily.Monospace, fontSize = 13.sp, background = p.codeFill)) { append("\u2009$code\u2009") }
                        i += code.length + 2
                    }
                    italic != null -> { withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { append(italic) }; i += italic.length + 2 }
                    link != null -> { withStyle(SpanStyle(color = p.accent)) { append(link.groupValues[1]) }; i += link.value.length }
                    else -> { append(s[i]); i++ }
                }
            }
        }
    }
}
