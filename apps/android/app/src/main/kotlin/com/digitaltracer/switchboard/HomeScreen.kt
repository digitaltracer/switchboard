package com.digitaltracer.switchboard

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Tune
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Approval
import com.digitaltracer.switchboard.api.models.Session
import com.digitaltracer.switchboard.api.models.TargetView
import com.digitaltracer.switchboard.core.AgentSpace
import com.digitaltracer.switchboard.core.ConversationSource
import com.digitaltracer.switchboard.core.FallbackPacer
import com.digitaltracer.switchboard.core.HerdrSession
import com.digitaltracer.switchboard.core.TileLines
import com.digitaltracer.switchboard.core.agentStatus
import com.digitaltracer.switchboard.core.machineId
import com.digitaltracer.switchboard.core.nameInSpace
import com.digitaltracer.switchboard.core.needsYou
import com.digitaltracer.switchboard.core.title
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** The paired app: home, and whatever it has opened on top. Back pops one level. */
@Composable
fun PairedRoot(model: AppModel, voice: Voice) {
    val listen = rememberListen(voice)
    BackHandler(model.path.isNotEmpty() && voice.phase == Voice.Phase.Idle) { model.pop() }
    BackHandler(voice.phase == Voice.Phase.Listening || voice.phase == Voice.Phase.Transcribing) { voice.cancel() }
    Box(Modifier.fillMaxSize()) {
        CompositionLocalProvider(LocalGoHome provides if (model.path.size > 1) model.path::clear else null) {
        when (val route = model.path.lastOrNull()) {
            null -> HomeScreen(model, voice, listen)
            is Route.SessionPage -> AgentScreen(model, voice, listen, ConversationSource.Session(route.id))
            is Route.AgentPage -> AgentScreen(model, voice, listen, ConversationSource.Agent(route.machine, route.name))
            is Route.Decision -> DecisionScreen(model, route)
            is Route.Space -> SpaceScreen(model, voice, route.key)
            is Route.MachinePage -> MachineSpacesScreen(model, route.machine)
            is Route.Diff -> DiffScreen(model, voice, route)
            Route.ShipSettings -> ShipSettingsScreen(model)
            Route.Machines -> MachinesScreen(model)
            Route.Settings -> SettingsScreen(model)
        }
        }
        model.banner?.let { b ->
            LaunchedEffect(b) {
                delay(6_000)
                if (model.banner == b) model.banner = null
            }
            Banner(
                b,
                Modifier.align(Alignment.TopCenter).statusBarsPadding().padding(horizontal = 16.dp, vertical = 8.dp)
                    .shadow(10.dp, androidx.compose.foundation.shape.RoundedCornerShape(12.dp))
                    .clickable { model.banner = null }.testTag("banner"),
            )
        }
        VoiceOverlay(voice)
    }
    if (voice.phase == Voice.Phase.Review && !voice.dictating) SendSheet(model, voice, listen)
}

enum class HomeTab(val label: String) { Now("Now"), Spaces("Spaces"), Projects("Projects") }

/** A live agent and the space it's in. */
private data class Item(val agent: HerdrSession.Agent, val space: AgentSpace) {
    val label get() = space.label(agent)
    val id: String
        get() = when (agent) {
            is HerdrSession.Agent.Task -> agent.session.id
            is HerdrSession.Agent.Live -> "${agent.agent.machineId}/${agent.agent.name}"
            is HerdrSession.Agent.Ended -> "${agent.agent.machineId}/${agent.agent.name}/${agent.agent.endedAt}"
        }
}

/**
 * Home: the hub's state, a sentence saying what needs you, then Now, Spaces and Projects. The ask
 * bar stays at the bottom: plus to start a session, a tap to type, the mic to speak.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(model: AppModel, voice: Voice, listen: Listen) {
    val tab = model.homeTab
    var starting by remember { mutableStateOf<String?>(null) }
    var refreshing by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    // The last line each busy agent said, by item id.
    val lastLines = remember { mutableStateMapOf<String, String>() }
    val pacer = remember { FallbackPacer() }
    val pager = rememberPagerState(initialPage = tab.ordinal) { HomeTab.entries.size }
    // A tapped tab scrolls the pager; a settled swipe picks the tab. The settled page, not the current
    // one, so passing over a middle page doesn't pick it.
    LaunchedEffect(tab) { if (pager.settledPage != tab.ordinal) pager.animateScrollToPage(tab.ordinal) }
    LaunchedEffect(pager) { snapshotFlow { pager.settledPage }.collect { model.homeTab = HomeTab.entries[it] } }
    // Connecting has taken long enough to say so.
    var slow by remember { mutableStateOf(false) }
    LaunchedEffect(model.loaded) {
        slow = false
        if (!model.loaded) {
            delay(8_000)
            slow = true
        }
    }

    val spaces = model.spaces(since = HerdrSession.startOfToday())
    val approvals: List<Pair<Session, Approval>> =
        model.sessions.flatMap { s -> s.pendingApprovals.map { s to it } }.sortedBy { it.second.deadline }
    val live = spaces.flatMap { sp -> sp.live.map { Item(it, sp) } }
    val waiting = live.filter { i ->
        i.agent.needsYou && (i.agent !is HerdrSession.Agent.Task || i.agent.session.pendingApprovals.isEmpty())
    }
    val working = live.filter { it.agent.agentStatus == "working" }
    val since = HerdrSession.startOfToday()
    val finished = live.filter { it.agent.agentStatus == "done" } +
        spaces.flatMap { sp -> sp.ended.filter { it is HerdrSession.Agent.Task && it.session.updatedAt >= since }.map { Item(it, sp) } }

    LaunchedEffect(model.wakes, model.foreground) {
        while (model.foreground) {
            model.refreshAgents(activity = true)
            val hub = model.hub ?: break
            // Each row's line comes from the hub's activity list; where that doesn't cover its machine, from
            // the conversation at most every 30 s. Reading the terminal would scroll it on the Mac.
            var reads = 0
            val now = model.spaces(since = HerdrSession.startOfToday()).flatMap { sp -> sp.live.map { Item(it, sp) } }
                .filter { it.agent.agentStatus == "working" || it.agent.agentStatus == "done" }
            for (t in now) {
                when (val known = model.tileLines?.line(t.agent) ?: TileLines.Line.Unknown) {
                    is TileLines.Line.Known -> {
                        val line = known.line
                        if (line == null) lastLines.remove(t.id) else lastLines[t.id] = line
                    }
                    TileLines.Line.Unknown -> {
                        if (reads >= 8 || !pacer.due(t.id)) continue
                        reads++
                        val page = when (val a = t.agent) {
                            is HerdrSession.Agent.Task -> runCatching { hub.transcript(a.session.id) }.getOrNull()
                            is HerdrSession.Agent.Live -> runCatching { hub.agentTranscript(a.agent.machineId, a.agent.name) }.getOrNull()
                            is HerdrSession.Agent.Ended -> null
                        } ?: continue
                        val last = page.items.lastOrNull { it.kind != "user" && it.text != null }?.text
                        last?.split("\n")?.firstOrNull { it.isNotBlank() }?.let { lastLines[t.id] = it }
                    }
                }
            }
            delay(5_000)
        }
    }

    InboxPage(
        top = {
            Row(
                Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.statusBars).height(44.dp).padding(start = 20.dp, end = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Row(Modifier.weight(1f).testTag("hubState"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    val (dot, text) = when {
                        model.connected -> palette.ok to hubLine(model)
                        model.unreachable -> palette.warn to "Can’t reach the hub · retrying"
                        else -> palette.mut to "Connecting…"
                    }
                    Dot(dot)
                    Text(text, style = Type.meta.copy(fontWeight = FontWeight.Medium), color = palette.sub, maxLines = 1)
                }
                Box(
                    Modifier.size(44.dp).clickable { model.path.add(Route.Settings) }.semantics { contentDescription = "Settings" }.testTag("devicesButton"),
                    Alignment.Center,
                ) { Icon(Icons.Outlined.Tune, null, Modifier.size(22.dp), tint = palette.ink) }
            }
        },
        bottom = {
            AskBar("Say or type what you want done", start = { starting = "" }, type = { voice.type(null) }, speak = { listen(null, false) })
        },
    ) {
        val connect = if (model.loaded) null else ConnectState.of(model, slow)
        val headline = if (connect != null) stateText(connect.main, connect.quiet) else when (tab) {
            HomeTab.Now -> {
                val needs = approvals.size + waiting.size
                if (needs == 0 && live.isEmpty() && finished.isEmpty()) {
                    stateText("Nothing running yet.", "Say what you want done.")
                } else {
                    stateText(
                        (if (model.away != null) "Welcome back. " else "") +
                            if (needs == 0) "Nothing needs you." else "$needs ${if (needs == 1) "thing needs" else "things need"} you.",
                        if (working.isEmpty()) "Nothing working." else "${working.size} ${if (working.size == 1) "agent" else "agents"} working.",
                    )
                }
            }
            HomeTab.Spaces -> {
                val all = model.spaces(since = null)
                val liveCount = all.count { !it.isEnded }
                val need = all.count { it.glyph == Glyph.Need }
                if (liveCount == 0) stateText("No spaces live.", if (all.isEmpty()) null else "${all.size} ended.")
                else stateText("$liveCount ${if (liveCount == 1) "space" else "spaces"} live.", if (need == 0) "None need you." else "$need need${if (need == 1) "s" else ""} you.")
            }
            HomeTab.Projects -> stateText("${model.projects.size} ${if (model.projects.size == 1) "project" else "projects"}.", "Tap one to start.")
        }
        StateLine(headline, Modifier.padding(start = 20.dp, end = 20.dp, top = 8.dp).testTag("headline"))
        Spacer(Modifier.height(14.dp))
        if (connect != null && model.unreachable) {
            HubUnreachable(model, connect)
            return@InboxPage
        }
        TextTabs(HomeTab.entries.map { it to it.label }, tab) { model.homeTab = it }
        PullToRefreshBox(
            refreshing,
            {
                scope.launch {
                    refreshing = true
                    model.startPolling()
                    model.refreshAgents(activity = true)
                    refreshing = false
                }
            },
            Modifier.weight(1f).fillMaxWidth(),
        ) {
            HorizontalPager(pager, Modifier.fillMaxSize(), beyondViewportPageCount = 1) { page ->
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, top = 14.dp, bottom = 24.dp)) {
                    // Until the hub's first answer, placeholders: empty lists would read as "nothing running".
                    if (!model.loaded) LoadingRows()
                    else when (HomeTab.entries[page]) {
                        HomeTab.Now -> Now(model, voice, listen, approvals, waiting, working, finished, live.isEmpty(), lastLines) { model.homeTab = it }
                        HomeTab.Spaces -> SpacesList(model)
                        HomeTab.Projects -> ProjectsList(model) { starting = it }
                    }
                }
            }
        }
    }
    starting?.let { project ->
        StartSheet(model, voice, project.ifEmpty { null }) { id ->
            starting = null
            id?.let { model.path.add(Route.SessionPage(it)) }
        }
    }
}

private fun hubLine(model: AppModel): String {
    val m = model.machines
    if (m.size == 1) return "${m[0].id} · connected"
    val d = m.firstOrNull { it.default }
    if (d != null && m.size > 1) return "${d.id} and ${m.size - 1} more · connected"
    return "Connected"
}

@Composable
private fun Now(
    model: AppModel, voice: Voice, listen: Listen,
    approvals: List<Pair<Session, Approval>>, waiting: List<Item>, working: List<Item>, finished: List<Item>,
    nothingLive: Boolean, lastLines: Map<String, String>, switchTab: (HomeTab) -> Unit,
) {
    model.away?.let { AwayCard(model, it) }
    model.lastSent?.takeIf { System.currentTimeMillis() - it.at < 600_000 }?.let { sent ->
        InboxRow(
            if (sent.started) "Started ${sent.target?.label ?: "a session"}" else "Sent to ${sent.target?.label ?: "the hub"}",
            "“${sent.text}”", Modifier.padding(bottom = 6.dp).testTag("sentCard"), first = true,
            onClick = { sent.target?.let { open(model, it) } },
            leading = { StatusGlyph(Glyph.Done) },
            trailing = { Text(Ago.short(sent.at), style = Type.meta, color = palette.mut) },
        )
    }
    approvals.firstOrNull()?.let { (s, a) ->
        Box(Modifier.padding(bottom = 6.dp)) { ApprovalCard(model, a, s, open = { model.path.add(Route.SessionPage(s.id)) }) }
    }
    approvals.drop(1).forEachIndexed { i, (s, a) ->
        InboxRow(
            when (a.kind) {
                "question" -> "${s.label} has a question"
                "plan" -> "${s.label} has a plan for you"
                else -> "${s.label} wants to run"
            },
            a.inputText, subtitleMono = a.kind == "permission", first = i == 0,
            onClick = { model.path.add(Route.SessionPage(s.id)) },
            leading = { StatusGlyph(Glyph.Need) }, trailing = { WhereTag(s.backend ?: "herdr") },
        )
    }
    waiting.forEachIndexed { i, item ->
        val a = item.agent
        val subtitle = when {
            a is HerdrSession.Agent.Task && a.session.hookFailed -> "Stuck at a dialog · answer it at its terminal"
            else -> a.title?.takeIf { it != a.nameInSpace }?.let { "Waiting for you · $it" } ?: "Waiting for you"
        }
        RenameMenu(model, a) { longPress ->
            InboxRow(
                item.label, subtitle, Modifier.testTag("needs-${item.label}"), subtitleColor = palette.warn, first = i == 0 && approvals.size <= 1,
                onLongClick = longPress,
                onClick = {
                    when (a) {
                        is HerdrSession.Agent.Live -> model.path.add(
                            Route.Decision(a.agent.machineId, a.agent.name, item.label, listOfNotNull(agentName(a.agent.kind), a.agent.machineId).joinToString(" · ")),
                        )
                        else -> a.route?.let { model.path.add(it) }
                    }
                },
                leading = { StatusGlyph(Glyph.Need) }, trailing = { WhereTag(item.space.backend) },
            )
        }
    }
    if (working.isNotEmpty()) {
        SectionHead("Working", "${working.size}")
        working.forEachIndexed { i, item ->
            val line = lastLines[item.id]
            RenameMenu(model, item.agent) { longPress ->
                InboxRow(
                    item.label, line ?: item.agent.title?.takeIf { it != item.agent.nameInSpace } ?: "Working",
                    Modifier.testTag("busy-${item.label}"), subtitleMono = line != null, first = i == 0,
                    onClick = item.agent.route?.let { r -> { model.path.add(r) } }, onLongClick = longPress,
                    leading = { StatusGlyph(Glyph.Work) }, trailing = { WhereTag(item.space.backend) },
                )
            }
        }
    }
    if (finished.isNotEmpty()) {
        SectionHead("Finished today", "${finished.size}")
        finished.forEachIndexed { i, item ->
            RenameMenu(model, item.agent) { longPress ->
                InboxRow(
                    item.label, lastLines[item.id] ?: item.agent.title ?: item.agent.subtitle, first = i == 0,
                    onClick = item.agent.route?.let { r -> { model.path.add(r) } }, onLongClick = longPress,
                    leading = { StatusGlyph(if (item.agent.isEnded) Glyph.End else Glyph.Done) },
                    trailing = {
                        val a = item.agent
                        if (a is HerdrSession.Agent.Task && item.agent.isEnded) Text(Ago.short(a.session.updatedAt), style = Type.meta, color = palette.mut)
                        else WhereTag(item.space.backend)
                    },
                )
            }
        }
    }
    if (nothingLive && finished.isEmpty() && approvals.isEmpty()) {
        val p = model.projects.map { it.id }
        SectionHead("Try saying")
        val first = p.firstOrNull()
        Example(
            if (first != null) "“In $first, fix the flaky retry test.”" else "“Fix the flaky retry test.”",
            if (first != null) "Starts a new task in $first, on its own branch" else "Starts a new task, on its own branch", true,
        ) {
            listen(null, false)
        }
        Example("“What is everyone working on?”", "Switchboard picks where it goes, and asks if unsure", false) { listen(null, false) }
        SectionHead("Ready on this hub")
        InboxRow(
            "${p.size} ${if (p.size == 1) "project" else "projects"}",
            if (p.size > 3) "${p.take(3).joinToString(", ")} and ${p.size - 3} more" else p.joinToString(", "),
            first = true, onClick = { switchTab(HomeTab.Projects) }, trailing = { Chevron() },
        )
        val ready = model.machines.filter { it.status == "ready" }.map { it.id }
        InboxRow(
            "${model.machines.size} ${if (model.machines.size == 1) "machine" else "machines"}",
            if (ready.isEmpty()) "None ready" else "${ready.joinToString(" and ")} ${if (ready.size == 1) "is" else "are"} ready",
            onClick = { model.path.add(Route.Machines) }, trailing = { Chevron() },
        )
        Hairline()
    } else if (approvals.isEmpty() && waiting.isEmpty() && working.isEmpty() && finished.isEmpty()) {
        val n = model.spaces(since = HerdrSession.startOfToday()).sumOf { it.live.size }
        StateRow("$n ${if (n == 1) "agent is" else "agents are"} idle. They're under Spaces.")
    }
}

@Composable
private fun Example(said: String, what: String, first: Boolean, onClick: () -> Unit) {
    Column(Modifier.fillMaxWidth().clickable(onClick = onClick)) {
        if (!first) Hairline()
        Column(Modifier.padding(vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(said, style = Type.title3, color = palette.ink)
            Text(what, style = Type.sub, color = palette.sub)
        }
    }
}

private fun open(model: AppModel, t: TargetView) {
    t.sessionId?.let { model.path.add(Route.SessionPage(it)) } ?: t.agent?.let { model.path.add(Route.AgentPage(t.machineId, it)) }
}
