package com.digitaltracer.switchboard

import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material.icons.outlined.Notifications
import androidx.compose.material.icons.outlined.NotificationsOff
import androidx.compose.material.icons.outlined.StopCircle
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Changes
import com.digitaltracer.switchboard.api.models.DeleteResponse
import com.digitaltracer.switchboard.api.models.Session
import com.digitaltracer.switchboard.core.AgentSpace
import com.digitaltracer.switchboard.core.ConversationSource
import com.digitaltracer.switchboard.core.HerdrSession
import com.digitaltracer.switchboard.core.Turns
import com.digitaltracer.switchboard.core.agentLabel
import com.digitaltracer.switchboard.core.agentTarget
import com.digitaltracer.switchboard.core.displayName
import com.digitaltracer.switchboard.core.holding
import com.digitaltracer.switchboard.core.isHerdrAgent
import com.digitaltracer.switchboard.core.rename
import java.util.UUID
import com.digitaltracer.switchboard.core.sessionTarget
import com.digitaltracer.switchboard.core.shownTitle
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

enum class AgentTab(val label: String) { Summary("Summary"), Conversation("Conversation"), Screen("Screen") }

/**
 * One agent: Summary, Conversation and Screen as tabs. A session Switchboard started adds its
 * approvals, changed files, details and a row of actions; one it didn't start has a message bar.
 */
@Composable
fun AgentScreen(model: AppModel, voice: Voice, listen: Listen, source: ConversationSource) {
    var tab by rememberSaveable(source) { mutableStateOf(AgentTab.Summary) }
    val convo = rememberConversation(source)
    var session by remember(source) { mutableStateOf<Session?>(null) }
    var changes by remember(source) { mutableStateOf<Changes?>(null) }
    // From the last action the user took; polls don't clear it.
    var error by remember(source) { mutableStateOf<String?>(null) }
    // The long-poll's own trouble, cleared once it gets through again.
    var pollError by remember(source) { mutableStateOf<String?>(null) }
    // What a delete did, kept on screen since the session leaves the lists.
    var notice by remember(source) { mutableStateOf<String?>(null) }
    var lifecycle by remember { mutableStateOf(false) }
    var details by remember { mutableStateOf(false) }
    var renaming by remember(source) { mutableStateOf(false) }
    // One request id per name tried, so a retry after a lost answer is the same request.
    val renameIds = remember(source) { mutableMapOf<String, String>() }
    val scope = rememberCoroutineScope()
    val scroll = rememberScrollState()
    val pager = rememberPagerState(initialPage = tab.ordinal) { AgentTab.entries.size }
    // A tapped tab scrolls the pager; a settled swipe picks the tab, so passing a middle page doesn't.
    LaunchedEffect(tab) { if (pager.settledPage != tab.ordinal) pager.animateScrollToPage(tab.ordinal) }
    LaunchedEffect(pager) { snapshotFlow { pager.settledPage }.collect { tab = AgentTab.entries[it] } }

    val sessionId = (source as? ConversationSource.Session)?.id
    val agentRef = source as? ConversationSource.Agent
    val info = agentRef?.let { a -> model.agents.firstOrNull { it.machineId == a.machine && it.name == a.name } }
    val s = session
    val machine = s?.machineId ?: agentRef?.machine.orEmpty()
    val backend = s?.backend ?: info?.backend ?: "herdr"
    val spaces = model.spaces(since = HerdrSession.startOfToday())
    val space: AgentSpace? = when {
        sessionId != null -> spaces.holding { it is HerdrSession.Agent.Task && it.session.id == sessionId }
        agentRef != null -> spaces.holding { it is HerdrSession.Agent.Live && it.agent.machineId == agentRef.machine && it.agent.name == agentRef.name }
        else -> null
    }
    val me = space?.agents?.firstOrNull {
        (it is HerdrSession.Agent.Live && it.agent.name == agentRef?.name) || (it is HerdrSession.Agent.Task && it.session.id == sessionId)
    }
    val title = s?.label ?: me?.let { space.label(it) } ?: info?.displayName ?: agentRef?.name ?: "Session"
    val kind = agentName(s?.agent ?: info?.kind)
    val target = s?.let(::sessionTarget) ?: agentRef?.let { agentTarget(it.machine, it.name, title) }
    val status = when {
        target != null && AppModel.saidKey(target) in model.awaiting -> SessionStatus.sent
        s != null -> SessionStatus.of(s)
        else -> info?.let { SessionStatus.agent(it.agentStatus) }
    }
    val running = s?.lifecycle == "running"
    val meta = listOfNotNull(kind, machine.ifEmpty { null }).joinToString(" · ")
    val pane: Route.Decision? = when {
        s != null -> s.agentName?.takeIf { s.lifecycle == "running" }?.let { Route.Decision(s.machineId, it, s.label, meta) }
        agentRef != null -> Route.Decision(agentRef.machine, agentRef.name, title, meta)
        else -> null
    }
    val watch = remember(pane?.machine, pane?.agent) { pane?.let { DialogWatch(it.machine, it.agent) } }

    suspend fun loadChanges() {
        val hub = model.hub ?: return
        // Older runners don't list changes; the section just stays hidden.
        if (sessionId != null) runCatching { hub.changes(sessionId) }.onSuccess { changes = it }
    }

    suspend fun refresh() {
        val hub = model.hub ?: return
        convo.refresh(hub)
        if (sessionId == null) return
        try {
            session = hub.session(sessionId)
            loadChanges()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message
        }
    }

    fun act(call: suspend () -> Unit) {
        scope.launch {
            try {
                call()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = e.message
            }
            refresh()
        }
    }

    // Long-polls until the screen goes away; each change also refreshes the changed files.
    LaunchedEffect(source, model.wakes, model.foreground) {
        if (sessionId == null) return@LaunchedEffect
        pollError = null
        var revision: Long? = null
        while (model.foreground) {
            val hub = model.hub ?: return@LaunchedEffect
            try {
                val next = hub.session(sessionId, after = revision)
                val changed = next.revision != revision
                session = next
                revision = next.revision
                pollError = null
                if (changed) loadChanges()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                pollError = e.message
                delay(3_000)
            }
        }
    }
    LaunchedEffect(source, model.wakes, model.foreground) { if (model.foreground) convo.follow(model) }
    LaunchedEffect(tab) { convo.showTerminal(tab == AgentTab.Screen) }
    LaunchedEffect(watch, model.wakes, model.foreground) { if (model.foreground) watch?.follow(model) }
    // Its status chip: only home refreshes agents otherwise.
    LaunchedEffect(source, model.wakes, model.foreground) {
        if (agentRef == null) return@LaunchedEffect
        while (model.foreground) {
            model.refreshAgents()
            delay(5_000)
        }
    }

    InboxPage(
        top = {
            if (tab == AgentTab.Summary) {
                PageTop(model.backLabel, model::pop, meta)
            } else {
                PageTop(model.backLabel, model::pop) {
                    Text(title, Modifier.weight(1f), style = Type.text.copy(fontWeight = FontWeight.Medium), color = palette.ink, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    status?.let { StatusChip(it) }
                }
            }
        },
        bottom = {
            if (s != null && tab == AgentTab.Summary) {
                Column(Modifier.fillMaxWidth().background(palette.bg)) {
                    Hairline()
                    Row(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        ActionButton(
                            if (running) Icons.Outlined.StopCircle else Icons.Outlined.Delete, if (running) "Stop" else "Delete",
                            "stopOrDelete", enabled = s.lifecycle !in setOf("starting", "deleted"), destructive = true,
                        ) { lifecycle = true }
                        Spacer(Modifier.weight(1f))
                        ActionButton(if (s.muted) Icons.Outlined.NotificationsOff else Icons.Outlined.Notifications, if (s.muted) "Muted" else "Notify") {
                            act { model.hub?.mute(s.id, !s.muted) }
                        }
                        Spacer(Modifier.weight(1f))
                        MicDisc(52.dp, "Talk to ${s.label}", "sessionMic", running) { listen(sessionTarget(s), false) }
                        Spacer(Modifier.weight(1f))
                        ActionButton(Icons.Outlined.Description, "Changes", enabled = changes?.files?.isNotEmpty() == true) {
                            scope.launch { scroll.animateScrollTo(scroll.maxValue) }
                        }
                        Spacer(Modifier.weight(1f))
                        ActionButton(Icons.Outlined.Keyboard, "Type", enabled = running) { voice.type(sessionTarget(s)) }
                    }
                }
            } else {
                AskBar(
                    "Message ${s?.label ?: info?.agentLabel ?: title}, or / for skills", dark = tab == AgentTab.Screen,
                    enabled = target != null && (s == null || running), type = { voice.type(target) }, speak = { listen(target, false) },
                )
            }
        },
    ) {
        if (tab == AgentTab.Summary) {
            Column(Modifier.padding(start = 20.dp, end = 20.dp, top = 4.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(title, style = Type.h1, color = palette.ink, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    status?.let { StatusChip(it) }
                    WhereTag(backend)
                    s?.branch?.let { Text("branch $it", Modifier.weight(1f, fill = false), style = Type.mono(12.5f), color = palette.sub, maxLines = 1) }
                    if (s?.muted == true) Icon(Icons.Outlined.NotificationsOff, "Muted", Modifier.size(16.dp), tint = palette.mut)
                }
            }
            Spacer(Modifier.padding(top = 8.dp))
        }
        TextTabs(AgentTab.entries.map { it to it.label }, tab) { tab = it }
        HorizontalPager(pager, Modifier.fillMaxWidth().weight(1f), beyondViewportPageCount = 1) { page ->
            when (AgentTab.entries[page]) {
                AgentTab.Conversation -> ConversationFeed(model, convo, kind ?: "Agent", status)
                AgentTab.Screen -> Screen(convo)
                AgentTab.Summary -> Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(start = 20.dp, end = 20.dp, bottom = 24.dp)) {
                    listOfNotNull(error?.let { it to palette.bad }, notice?.let { it to palette.sub }, pollError?.let { it to palette.warn }, s?.error?.let { it to palette.bad })
                        .forEach { (text, color) -> Note(text, Modifier.padding(top = 14.dp).then(if (text == notice) Modifier.testTag("deleteNotice") else Modifier), color) }
                    if (s?.hookFailed == true) Banner("It's stuck at a dialog Switchboard won't answer. Answer it at its terminal.", Modifier.padding(top = 14.dp))
                    target?.let { model.said[AppModel.saidKey(it)] }?.let { Box(Modifier.padding(top = 14.dp)) { SaidBubble(it) } }
                    val act = model.away
                    model.awayFor(machine, s?.agentName ?: (source as? ConversationSource.Agent)?.name, sessionId)?.let { mine ->
                        // Quoted only when "Latest reply" can't show it.
                        val shown = !convo.ended && convo.unavailable == null &&
                            (!convo.conversation.isLoaded || Turns.latest(convo.conversation.items).reply != null)
                        if (act != null) AwayTimeline(mine, act.since, act.now, if (shown) null else mine.lastReply)
                    }
                    s?.pendingApprovals?.forEach { a -> Box(Modifier.padding(top = 14.dp)) { ApprovalCard(model, a, s, decided = { refresh() }) } }
                    if (watch != null && pane != null) {
                        Box(Modifier.padding(top = 14.dp)) { WaitingCard(watch, { model.path.add(pane) }) { tab = AgentTab.Screen } }
                    }
                    info?.shownTitle?.let {
                        SectionHead("Working on")
                        Text(it, style = Type.title3, color = palette.ink)
                    }
                    val c = changes
                    LatestReply(convo, hideFiles = c != null && c.files.isNotEmpty()) { tab = AgentTab.Conversation }
                    if (c != null && c.files.isNotEmpty()) {
                        val open = sessionId?.let { id -> { i: Int -> model.path.add(Route.Diff(id, c.files.map { it.path }, i)); Unit } }
                        val added = c.files.sumOf { it.added ?: 0 }
                        val deleted = c.files.sumOf { it.deleted ?: 0 }
                        SectionHead(
                            "${c.title()} · +$added −$deleted",
                            action = open?.let { o -> { Text("Review all", Modifier.clickable { o(0) }.testTag("reviewAll"), style = Type.meta.copy(fontWeight = FontWeight.Medium), color = palette.ac) } },
                        )
                        Column(Modifier.fillMaxWidth().card(16.dp, palette.sf).border(1.dp, palette.line, androidx.compose.foundation.shape.RoundedCornerShape(16.dp)).padding(horizontal = 14.dp)) {
                            ChangesList(c, open)
                        }
                    }
                    val shipTarget = s?.let(::sessionTarget) ?: (source as? ConversationSource.Agent)?.let { com.digitaltracer.switchboard.api.models.TargetView("agent", it.machine, info?.agentLabel ?: it.name, null, it.name) }
                    if (shipTarget != null) {
                        val latest = Turns.latest(convo.conversation.items)
                        ShipCard(
                            model, shipTarget, s?.projectId, s?.branch, s?.agentStatus ?: info?.agentStatus ?: "unknown",
                            latest.reply?.text, latest.at?.let { runCatching { java.time.Instant.parse(it).toEpochMilli() }.getOrNull() },
                        )
                    }
                    ConversationRows(convo, { tab = AgentTab.Conversation }) { tab = AgentTab.Screen }
                    if (s != null) {
                        SectionHead("Details", action = { DetailsLink(details, "Show", "Hide") { details = !details } })
                    }
                    if (details && s != null) {
                        FieldRow("Project", s.projectId, first = true)
                        FieldRow("Machine", "") {
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                Dot(if (s.machineStatus == "ready") palette.ok else palette.warn)
                                Text("${s.machineId} · ${s.machineStatus}", style = Type.field, color = palette.sub)
                            }
                        }
                        s.branch?.let { FieldRow("Branch", it, mono = true) }
                        Hairline()
                    }
                    // The name a rename changes: a session's label, or an agent's own name without its space.
                    val own = s?.let { HerdrSession.Agent.Task(it) } ?: info?.let { HerdrSession.Agent.Live(it) }
                    AtYourDesk(backend, machine, space, s?.herdrSession ?: info?.herdrSession, live = s?.live ?: true) {
                        own?.let { NameRow(s?.label ?: info?.agentLabel ?: title, it.rename) { renaming = true } }
                    }
                }
            }
        }
    }

    if (renaming) {
        val name = s?.label ?: info?.agentLabel ?: title
        val herdrAgent = info?.let { HerdrSession.Agent.Live(it).isHerdrAgent } ?: false
        RenameSheet("Rename $name", name, herdrAgent, dismiss = { renaming = false }) { new ->
            val hub = model.hub ?: return@RenameSheet
            if (sessionId != null) {
                hub.renameSession(sessionId, new)
                refresh()
                session?.let(model::renamedSession)
            } else if (agentRef != null) {
                val listed = hub.renameAgent(agentRef.machine, agentRef.name, new, renameIds.getOrPut(new) { UUID.randomUUID().toString() })
                model.renamedAgent(agentRef.machine, agentRef.name, listed)
                model.refreshAgents()
                model.replaceTop(Route.AgentPage(agentRef.machine, listed))
            }
        }
    }

    if (lifecycle && s != null) {
        val delete = Triple<String, String?, () -> Unit>(
            "Delete",
            if (running) "Also removes its Herdr session. The worktree goes only if it is clean and merged. The branch is always kept."
            else "Removes its Herdr session. The worktree goes only if it is clean and merged. The branch is always kept.",
        ) { act { model.hub?.let { notice = describe(it.delete(s.id)) } } }
        ChoiceSheet(
            if (running) "Stop or delete ${s.label}?" else "Delete ${s.label}?",
            listOfNotNull(
                Triple<String, String?, () -> Unit>("Stop", "Ends the agent. The worktree and branch stay.") { act { model.hub?.stop(s.id) } }
                    .takeIf { running },
                delete,
            ),
        ) { lifecycle = false }
    }
}

@Composable
private fun DetailsLink(open: Boolean, show: String = "Details", hide: String = "Hide details", modifier: Modifier = Modifier, toggle: () -> Unit) {
    Text(if (open) hide else show, modifier.clickable(onClick = toggle).padding(start = 8.dp), style = Type.meta.copy(fontWeight = FontWeight.Medium), color = palette.ac)
}

/** The latest reply, its tags, and the ways into the whole conversation and the screen. */
@Composable
private fun LatestReply(convo: ConversationModel, hideFiles: Boolean, open: () -> Unit) {
    val latest = Turns.latest(convo.conversation.items)
    SectionHead("Latest reply", latest.at?.let(Turns::time))
    Column(Modifier.testTag("latest")) {
        val why = convo.unavailable
        val reply = latest.reply?.text
        when {
            convo.ended -> Note("This session has ended.", color = palette.sub)
            why != null -> Note("No conversation to show ($why). Its screen is under Screen.", color = palette.sub)
            !convo.conversation.isLoaded -> ConversationSkeleton(rows = 1)
            reply != null -> Box(Modifier.clickable(onClick = open)) { Clamped(168.dp) { MarkdownText(reply) } }
            else -> Note("Nothing said yet.", color = palette.sub)
        }
    }
    // A session lists its changed files right below, so the files chip would only repeat it.
    val tags = if (hideFiles) latest.tags.filter { "file" !in it.text } else latest.tags
    if (tags.isNotEmpty()) Row(Modifier.padding(top = 14.dp)) { TagRow(tags) }
}

@Composable
private fun ConversationRows(convo: ConversationModel, conversation: () -> Unit, screen: () -> Unit) {
    Column(Modifier.padding(top = 18.dp)) {
        if (convo.unavailable == null && !convo.ended) {
            InboxRow("Read the whole conversation", modifier = Modifier.testTag("openConversation"), first = true, onClick = conversation, trailing = { Chevron() })
        }
        if (!convo.ended) {
            InboxRow("See its terminal", modifier = Modifier.testTag("openScreen"), first = convo.unavailable != null, onClick = screen, trailing = { Chevron() })
        }
        Hairline()
    }
}

/** Where it runs, the space to look under, and for a terminal the command to attach. */
@Composable
fun AtYourDesk(
    backend: String, machine: String, space: AgentSpace?, herdrSession: String?, live: Boolean = true,
    more: @Composable () -> Unit = {},
) {
    val app = backend == "hc"
    var open by remember { mutableStateOf(false) }
    SectionHead("At your desk", action = { DetailsLink(open, "Show", "Hide", Modifier.testTag("deskToggle")) { open = !open } })
    if (!open) return
    FieldRow("Where", "${if (app) "Herdr app" else "Herdr terminal"} on $machine", first = true)
    if (app) {
        space?.key?.space?.let { FieldRow("Space", it) }
    } else if (live) {
        (space?.attach ?: herdrSession)?.let { attach ->
            space?.key?.space?.let { FieldRow("Workspace", it) }
            FieldRow("Attach", "") { SelectionContainer { Text("herdr session attach $attach", style = Type.mono(12.5f), color = palette.sub, maxLines = 1) } }
        }
    }
    more()
    Hairline()
}

private fun describe(r: DeleteResponse): String {
    val branch = r.branch?.let { " Branch $it is kept." }.orEmpty()
    return when {
        r.alreadyDeleted -> "Already deleted."
        r.worktreeRemoved -> "Deleted, with its worktree.$branch"
        r.keptBecause != null -> "Deleted. Its worktree was kept: ${r.keptBecause}.$branch"
        else -> "Deleted.$branch"
    }
}
