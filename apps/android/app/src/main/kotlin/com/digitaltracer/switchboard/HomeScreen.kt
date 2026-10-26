package com.digitaltracer.switchboard

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import com.digitaltracer.switchboard.api.models.TargetView
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Computer
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.Inbox
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material.icons.outlined.WifiOff
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitaltracer.switchboard.api.models.Session
import com.digitaltracer.switchboard.core.displayName
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** The paired app: home, and whatever it has opened on top. Back pops one level. */
@Composable
fun PairedRoot(model: AppModel, voice: Voice) {
    val listen = rememberListen(voice)
    BackHandler(model.path.isNotEmpty() && voice.phase == Voice.Phase.Idle) { model.pop() }
    BackHandler(voice.phase == Voice.Phase.Listening) { voice.cancel() }
    Box(Modifier.fillMaxSize()) {
        when (val route = model.path.lastOrNull()) {
            null -> HomeScreen(model, voice, listen)
            is Route.SessionPage -> SessionScreen(model, voice, listen, route.id)
            is Route.AgentPage -> AgentScreen(model, voice, listen, route.machine, route.name)
            Route.AllSessions -> AllSessionsScreen(model)
            is Route.MachinePage -> AllSessionsScreen(model, route.machine)
            Route.Machines -> MachinesScreen(model)
            Route.Projects -> ProjectsScreen(model)
            Route.Settings -> SettingsScreen(model)
            is Route.Conversation -> ConversationScreen(model, route.source, route.title)
        }
        VoiceOverlay(voice)
    }
    if (voice.phase == Voice.Phase.Review) SendSheet(model, voice, listen)
}

/** Home: what needs you, what's running, then lists. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(model: AppModel, voice: Voice, listen: Listen) {
    val sessions = model.sessions
    val waiting = sessions.filter { it.pendingApprovals.isNotEmpty() }
    val running = sessions.filter { it.live && it.pendingApprovals.isEmpty() }
    val since = System.currentTimeMillis() - 86_400_000
    val finished = sessions.filter { !it.live && it.updatedAt > since }
    // The last line each running session printed, by session id.
    val lastLines = remember { mutableStateMapOf<String, String>() }
    val scope = rememberCoroutineScope()
    var refreshing by remember { mutableStateOf(false) }
    var starting by remember { mutableStateOf(false) }
    val runningNow by rememberUpdatedState(running)

    LaunchedEffect(model.wakes) {
        while (true) {
            model.refreshAgents()
            val hub = model.hub ?: break
            for (s in runningNow.take(8).filter { it.lifecycle == "running" }) {
                val text = runCatching { hub.output(s.id, lines = 6).text }.getOrNull() ?: continue
                text.split("\n").lastOrNull { it.isNotBlank() }?.let { lastLines[s.id] = it }
            }
            delay(6_000)
        }
    }

    Page(back = null, actions = {
        BarIcon(Icons.Filled.Add, "Start a session", "startButton") { starting = true }
        BarIcon(Icons.Outlined.Settings, "Settings", "devicesButton") { model.path.add(Route.Settings) }
    }) {
      Box(Modifier.fillMaxSize()) {
        PullToRefreshBox(
            refreshing,
            {
                scope.launch {
                    refreshing = true
                    model.startPolling()
                    model.refreshAgents()
                    refreshing = false
                }
            },
            Modifier.fillMaxSize(),
        ) {
            LazyColumn(
                Modifier.fillMaxSize().testTag("sessionsList"),
                contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 170.dp),
                verticalArrangement = Arrangement.spacedBy(24.dp),
            ) {
                item {
                    Column {
                        LargeTitle("Agents", horizontal = 0)
                        Summary(model.connected, waiting.size, running.size)
                    }
                }
                model.lastSent?.takeIf { System.currentTimeMillis() - it.at < 600_000 }?.let { sent ->
                    item { SentCard(sent) { t -> open(model, t) } }
                }
                for (s in waiting) {
                    items(s.pendingApprovals, key = { it.id }) { a ->
                        ApprovalCard(model, a, s, open = { model.path.add(Route.SessionPage(s.id)) })
                    }
                }
                if (running.isNotEmpty()) {
                    item {
                        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            Text("Running", style = Type.title3, color = palette.label)
                            LazyRow(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                items(running, key = { it.id }) { s ->
                                    RunningTile(s, lastLines[s.id]) { model.path.add(Route.SessionPage(s.id)) }
                                }
                            }
                        }
                    }
                }
                if (sessions.isEmpty() && model.agents.isEmpty()) item { Empty() }
                if (finished.isNotEmpty()) {
                    item {
                        InsetGroup("Finished today") {
                            finished.forEachIndexed { i, s ->
                                val status = SessionStatus.of(s)
                                ListRow(status.symbol, status.color, s.label, "${status.text} · ${s.projectId}", Ago.short(s.updatedAt), i == 0) {
                                    model.path.add(Route.SessionPage(s.id))
                                }
                            }
                        }
                    }
                }
                if (model.agents.isNotEmpty()) {
                    item {
                        InsetGroup("Started in Herdr") {
                            model.agents.forEachIndexed { i, a ->
                                val where = if (a.displayName == a.name) a.machineId else a.herdrSession ?: a.machineId
                                ListRow(
                                    Icons.Outlined.Terminal, palette.secondary, a.displayName,
                                    "${SessionStatus.agent(a.agentStatus).text} · $where", "", i == 0,
                                ) { model.path.add(Route.AgentPage(a.machineId, a.name)) }
                            }
                        }
                    }
                }
                item {
                    InsetGroup("Browse") {
                        ListRow(Icons.Outlined.Inbox, palette.secondary, "All sessions", null, "${sessions.size + model.agents.size}", true) {
                            model.path.add(Route.AllSessions)
                        }
                        ListRow(Icons.Outlined.Computer, palette.accent, "Machines", null, "", false) { model.path.add(Route.Machines) }
                        ListRow(Icons.Outlined.Folder, palette.accent, "Projects", null, "", false) { model.path.add(Route.Projects) }
                    }
                }
            }
        }
        Dock(voice, listen)
      }
    }
    if (starting) {
        StartSheet(model) { id ->
            starting = false
            id?.let { model.path.add(Route.SessionPage(it)) }
        }
    }
}

private fun open(model: AppModel, t: TargetView) {
    t.sessionId?.let { model.path.add(Route.SessionPage(it)) } ?: t.agent?.let { model.path.add(Route.AgentPage(t.machineId, it)) }
}

/** The mic is the main way in; the keyboard is beside it. */
@Composable
private fun BoxScope.Dock(voice: Voice, listen: Listen) {
    Column(
        Modifier.align(Alignment.BottomCenter).fillMaxWidth()
            .background(Brush.verticalGradient(0f to palette.background.copy(alpha = 0f), 0.4f to palette.background))
            .windowInsetsPadding(WindowInsets.navigationBars)
            .padding(start = 16.dp, end = 16.dp, top = 36.dp, bottom = 8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text("Tap and say what you want done", style = Type.footnote, color = palette.secondary)
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(
                Modifier.size(50.dp).shadow(6.dp, CircleShape).background(palette.card, CircleShape)
                    .clickable { voice.type(null) }.semantics { contentDescription = "Type instead" }.testTag("typeButton"),
                Alignment.Center,
            ) { Icon(Icons.Outlined.Keyboard, null, Modifier.size(24.dp), tint = palette.label) }
            Spacer(Modifier.weight(1f))
            MicButton(voice, 72.dp, "Talk to your agents", "micButton") { listen(null, false) }
            Spacer(Modifier.weight(1f))
            Spacer(Modifier.size(50.dp))
        }
    }
}

@Composable
private fun SentCard(sent: AppModel.Sent, open: (TargetView) -> Unit) {
    Column(
        Modifier.fillMaxWidth().card(20.dp, palette.card).clickable { sent.target?.let(open) }.padding(16.dp).testTag("sentCard"),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(Icons.Filled.Check, null, Modifier.size(16.dp), tint = palette.done)
            Text(
                if (sent.started) "Started ${sent.target?.label ?: "a session"}" else "Sent to ${sent.target?.label ?: "the hub"}",
                style = Type.footnote, fontWeight = FontWeight.SemiBold, color = palette.done,
            )
        }
        Text(sent.text, Modifier.card(18.dp, palette.accent).padding(horizontal = 13.dp, vertical = 9.dp), style = Type.body, color = Color.White)
    }
}

@Composable
private fun Summary(connected: Boolean, waiting: Int, running: Int) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
        if (connected) {
            Dot(palette.done)
            val parts = listOfNotNull(if (waiting > 0) "$waiting need${if (waiting == 1) "s" else ""} you" else null, "$running running")
            Text(parts.joinToString(" · "), style = Type.subheadline, color = palette.secondary)
        } else {
            Icon(Icons.Outlined.WifiOff, null, Modifier.size(16.dp), tint = palette.needs)
            Text("Can't reach the hub; retrying.", style = Type.subheadline, color = palette.secondary)
        }
    }
}

@Composable
private fun Empty() {
    Column(Modifier.fillMaxWidth().card(20.dp, palette.card).padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text("Nothing running yet", style = Type.headline, color = palette.label)
        Text("Tap the mic and say what to do, like “In kraft, fix the flaky retry test.”", style = Type.body, color = palette.secondary)
    }
}

@Composable
private fun RunningTile(session: Session, lastLine: String?, open: () -> Unit) {
    val status = SessionStatus.of(session)
    Column(
        Modifier.size(width = 168.dp, height = 150.dp).card(20.dp, palette.card).clickable(onClick = open).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Dot(status.color, 7.dp)
            Text("${status.text} · ${Ago.short(session.updatedAt)}", style = Type.caption, fontWeight = FontWeight.SemiBold, color = status.color)
        }
        Text(session.label, style = Type.subheadline, fontWeight = FontWeight.SemiBold, color = palette.label, maxLines = 1)
        Text("${session.projectId} · ${session.machineId}", style = Type.footnote, color = palette.secondary, maxLines = 1)
        Spacer(Modifier.weight(1f))
        Text(lastLine.orEmpty(), color = palette.secondary, fontFamily = FontFamily.Monospace, fontSize = 11.sp, lineHeight = 14.sp, maxLines = 2)
    }
}
