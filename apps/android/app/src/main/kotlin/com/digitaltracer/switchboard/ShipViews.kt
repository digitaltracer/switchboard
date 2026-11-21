package com.digitaltracer.switchboard

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.CallSplit
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.ArrowDownward
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.outlined.AddCircleOutline
import androidx.compose.material.icons.outlined.RemoveCircleOutline
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Skill
import com.digitaltracer.switchboard.api.models.TargetView
import com.digitaltracer.switchboard.core.PullRequest
import com.digitaltracer.switchboard.core.ShipButton
import com.digitaltracer.switchboard.core.ShipButtons
import com.digitaltracer.switchboard.core.ShipOutcome
import java.util.UUID
import kotlinx.coroutines.launch

/**
 * "Ship it": buttons that ask a finished agent to run one of the owner's own skills. Switchboard
 * never runs git or gh; a push still comes to the phone as an approval.
 */
@Composable
fun ShipCard(model: AppModel, target: TargetView, project: String?, branch: String?, status: String, reply: String?, replyAt: Long?) {
    var skills by remember(target) { mutableStateOf<List<Skill>>(emptyList()) }
    var confirming by remember { mutableStateOf<ShipButton?>(null) }
    var more by remember { mutableStateOf(false) }
    LaunchedEffect(target) {
        val hub = model.hub ?: return@LaunchedEffect
        skills = runCatching {
            target.sessionId?.let { hub.skills(it) } ?: target.agent?.let { hub.agentSkills(target.machineId, it) } ?: emptyList()
        }.getOrDefault(emptyList())
    }
    val key = AppModel.saidKey(target)
    val finished = status == "idle" || status == "done"
    val set = model.shipButtons?.let { ShipButtons.effective(it, project) }.orEmpty()
    val buttons = set.ifEmpty { ShipButtons.suggested(skills.map { it.name }) }
    val sent = model.lastShip[key]
    val outcome = sent?.let {
        val after = replyAt != null && replyAt >= it.at
        ShipOutcome.of(it.button, finished && after, if (after) reply else null)
    }
    val pr = (outcome as? ShipOutcome.Opened)?.pr ?: reply?.let(PullRequest::find)

    Column(Modifier.padding(top = 14.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        if (sent != null && outcome == ShipOutcome.Running) Note("${target.label} is running ${sent.button.skill}…")
        if (outcome == ShipOutcome.NoPullRequest) Note("No pull request was opened. See the agent's reply.", Modifier.testTag("noPullRequest"), palette.warn)
        if (pr != null) {
            PrCard(pr)
            val next = buttons.filter { it.afterPr }
            if (next.isNotEmpty() && finished) {
                SectionHead("Next, with your skills", top = 0.dp)
                ShipGrid(next) { confirming = it }
            }
        } else if (finished && outcome != ShipOutcome.Running) {
            val main = buttons.filter { !it.afterPr }
            Column(
                Modifier.fillMaxWidth().card(18.dp, palette.sf).border(1.dp, palette.line, RoundedCornerShape(18.dp)).padding(16.dp).testTag("shipIt"),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Ship it", Modifier.weight(1f), style = Type.title.copy(fontWeight = FontWeight.SemiBold), color = palette.ink)
                    Text("Your skills", style = Type.meta, color = palette.mut)
                }
                if (main.isNotEmpty()) ShipGrid(main) { confirming = it }
                if (skills.isNotEmpty()) {
                    Box {
                        Text("Run another skill…", Modifier.clickable { more = true }.testTag("runAnotherSkill"), style = Type.title, color = palette.ac)
                        DropdownMenu(more, { more = false }) {
                            skills.forEach { s ->
                                DropdownMenuItem({ Text(s.name, style = Type.mono(14f)) }, {
                                    more = false
                                    confirming = ShipButton(s.name, ShipButtons.label(s.name))
                                })
                            }
                        }
                    }
                }
                Hairline()
                Text(
                    "Switchboard asks the agent to run your skill. A push, or anything else that needs your OK, still comes to you as an approval.",
                    style = Type.meta, color = palette.sub,
                )
                buttons.firstOrNull { it.afterPr }?.let { Text("${it.label} appears once a pull request is open.", style = Type.meta, color = palette.mut) }
            }
        }
    }
    confirming?.let { b ->
        ShipConfirmSheet(model, b.invocation(skills.map { it.name }), target, branch) { confirming = null }
    }
}

/** The main button full width and black, the rest two to a row. */
@Composable
private fun ShipGrid(list: List<ShipButton>, pick: (ShipButton) -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        list.firstOrNull()?.let { b ->
            Row(
                Modifier.fillMaxWidth().heightIn(min = 56.dp).clip14().background(palette.ink).clickable { pick(b) }
                    .padding(horizontal = 16.dp).testTag("ship-${b.skill}"),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(Icons.AutoMirrored.Filled.CallSplit, null, Modifier.size(18.dp), tint = palette.inv)
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(b.label, style = Type.title.copy(fontWeight = FontWeight.SemiBold), color = palette.inv)
                    Text(b.skill, style = Type.mono(12f), color = palette.inv.copy(alpha = 0.7f))
                }
                Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, null, tint = palette.inv)
            }
        }
        list.drop(1).chunked(2).forEach { row ->
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                row.forEach { b ->
                    Column(
                        Modifier.weight(1f).heightIn(min = 56.dp).clip14().background(palette.sf)
                            .border(BorderStroke(1.5.dp, palette.line), RoundedCornerShape(14.dp)).clickable { pick(b) }
                            .padding(horizontal = 14.dp, vertical = 9.dp).testTag("ship-${b.skill}"),
                    ) {
                        Text(b.label, style = Type.title.copy(fontWeight = FontWeight.SemiBold), color = palette.ink, maxLines = 1)
                        Text(b.skill, style = Type.mono(12f), color = palette.mut, maxLines = 1)
                    }
                }
                if (row.size == 1) Spacer(Modifier.weight(1f))
            }
        }
    }
}

private fun Modifier.clip14() = this.then(Modifier.card(14.dp, androidx.compose.ui.graphics.Color.Transparent))

/** A pull request found in the agent's reply, with a button that opens it. */
@Composable
fun PrCard(pr: PullRequest) {
    val context = LocalContext.current
    Column(
        Modifier.fillMaxWidth().card(18.dp, palette.sf).border(1.dp, palette.line, RoundedCornerShape(18.dp)).padding(16.dp).testTag("pullRequest"),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(36.dp).card(10.dp, palette.ok.copy(alpha = 0.12f)), Alignment.Center) {
                Icon(Icons.AutoMirrored.Filled.CallSplit, null, tint = palette.ok)
            }
            Spacer(Modifier.width(12.dp))
            Column {
                Text("Pull request opened", style = Type.title.copy(fontWeight = FontWeight.SemiBold), color = palette.ink)
                Text("${pr.repo} · #${pr.number}", style = Type.meta, color = palette.sub)
            }
        }
        pr.title?.let { Text(it, style = Type.text.copy(fontWeight = FontWeight.Medium), color = palette.ink) }
        Text(pr.url.removePrefix("https://"), style = Type.mono(12.5f), color = palette.sub, maxLines = 1, overflow = TextOverflow.MiddleEllipsis)
        BigButton("Open pull request", Modifier.fillMaxWidth().testTag("openPullRequest"), ButtonKind.Outline) {
            context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(pr.url)))
        }
        Text("Found in the agent's reply. Switchboard doesn't call GitHub.", style = Type.meta, color = palette.mut)
    }
}

/** Confirms a ship skill, with optional instructions after it, then sends it as a message. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ShipConfirmSheet(model: AppModel, button: ShipButton, target: TargetView, branch: String?, done: () -> Unit) {
    val scope = rememberCoroutineScope()
    var instructions by remember { mutableStateOf("") }
    var sending by remember { mutableStateOf(false) }
    var note by remember { mutableStateOf<String?>(null) }
    val requestId = remember { UUID.randomUUID().toString() }
    ModalBottomSheet(
        onDismissRequest = done,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = palette.bg,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
    ) {
        Column(Modifier.fillMaxHeight(0.9f).imePadding().navigationBarsPadding().padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                Text("Run ${button.skill} on ${target.label}?", style = Type.title3.copy(fontWeight = FontWeight.SemiBold), color = palette.ink)
                Text(
                    "The agent runs your ${button.skill} skill${branch?.let { " on branch $it" } ?: ""}.",
                    style = Type.text, color = palette.sub,
                )
                SectionHead("Instructions for the skill (optional)", top = 0.dp)
                BasicTextField(
                    instructions, { instructions = it },
                    Modifier.fillMaxWidth().card(12.dp, palette.sf).border(1.dp, palette.line, RoundedCornerShape(12.dp)).padding(14.dp).testTag("shipInstructions"),
                    textStyle = Type.field.copy(color = palette.ink), singleLine = true, cursorBrush = SolidColor(palette.ac),
                )
                SectionHead("Message the agent gets", top = 0.dp)
                Text(button.message(instructions), Modifier.fillMaxWidth().card(12.dp, palette.code).padding(14.dp), style = Type.mono(13.5f), color = androidx.compose.ui.graphics.Color(0xFFF2F3F4))
                Note(
                    "It runs with the agent's own permissions. git push and other commands that need your OK go to Needs you, signed with your fingerprint or screen lock.",
                    Modifier.fillMaxWidth().card(12.dp, palette.sf).border(1.dp, palette.line, RoundedCornerShape(12.dp)).padding(14.dp),
                    palette.sub,
                )
                note?.let { Note(it, color = palette.bad) }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                BigButton("Cancel", Modifier.width(130.dp), ButtonKind.Outline, onClick = done)
                BigButton(if (sending) "Sending…" else "Run ${button.skill}", Modifier.weight(1f).testTag("shipRun"), enabled = !sending, busy = sending) {
                    scope.launch {
                        val hub = model.hub ?: return@launch
                        sending = true
                        val text = button.message(instructions)
                        runCatching { hub.message(text, requestId, null, target = target) }
                            .onSuccess { r ->
                                if (r.outcome == "sent") {
                                    model.lastShip = model.lastShip + (AppModel.saidKey(target) to AppModel.SentShip(button, System.currentTimeMillis()))
                                    model.record(text, r.current ?: target, started = false)
                                    done()
                                } else {
                                    note = r.message
                                }
                            }
                            .onFailure { note = it.message }
                        sending = false
                    }
                }
            }
        }
    }
}

/** Settings › Ship actions: which skills show as buttons, for all projects or one. Kept on the hub. */
@Composable
fun ShipSettingsScreen(model: AppModel) {
    val scope = rememberCoroutineScope()
    var target by remember { mutableStateOf("*") }
    var list by remember { mutableStateOf<List<ShipButton>>(emptyList()) }
    var skills by remember { mutableStateOf<List<Skill>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var pickProject by remember { mutableStateOf(false) }
    fun load() {
        val b = model.shipButtons ?: return
        list = if (target == "*") b.all.map(::ShipButton) else b.projects.firstOrNull { it.project == target }?.buttons?.map(::ShipButton).orEmpty()
    }
    fun save(next: List<ShipButton>) {
        list = next
        scope.launch {
            val hub = model.hub ?: return@launch
            runCatching { hub.setShipButtons(target, next) }
                .onSuccess { error = null; model.loadShipButtons() }
                .onFailure { error = it.message }
        }
    }
    LaunchedEffect(Unit) {
        model.loadShipButtons()
        load()
        val hub = model.hub ?: return@LaunchedEffect
        val seen = linkedMapOf<String, Skill>()
        model.sessions.filter { it.lifecycle == "running" }.take(3).forEach { s ->
            runCatching { hub.skills(s.id) }.getOrDefault(emptyList()).forEach { seen.putIfAbsent(it.name, it) }
        }
        skills = seen.values.sortedBy { it.name }
    }
    LaunchedEffect(target) { load() }
    InboxPage(top = { PageTop(model.backLabel, model::pop) }) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).padding(bottom = 24.dp)) {
            Text("Ship actions", style = Type.h1, color = palette.ink)
            Text("Buttons on a finished session. Each one asks the agent to run one of its own skills.", Modifier.padding(top = 8.dp), style = Type.text, color = palette.sub)
            Row(Modifier.padding(top = 16.dp).fillMaxWidth().card(12.dp, palette.fill).padding(3.dp)) {
                listOf("*" to "All projects", "one" to if (target == "*") "One project" else "$target only").forEach { (k, label) ->
                    val on = (k == "*") == (target == "*")
                    Box(Modifier.weight(1f)) {
                        Text(
                            label,
                            Modifier.fillMaxWidth().card(10.dp, if (on) palette.sf else androidx.compose.ui.graphics.Color.Transparent)
                                .clickable { if (k == "*") target = "*" else pickProject = true }.padding(vertical = 10.dp),
                            style = Type.title, color = if (on) palette.ink else palette.sub, textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                        )
                        if (k != "*") DropdownMenu(pickProject, { pickProject = false }) {
                            model.projects.forEach { p -> DropdownMenuItem({ Text(p.id) }, { pickProject = false; target = p.id }) }
                        }
                    }
                }
            }
            error?.let { Note(it, Modifier.padding(top = 10.dp), palette.bad) }
            SectionHead("Shown, in this order")
            if (list.isEmpty()) {
                Note(if (target == "*") "None yet. Until you add some, a finished agent suggests your usual ship skills." else "None of its own: $target shows the buttons for all projects.")
            }
            val main = list.indexOfFirst { !it.afterPr }
            list.forEachIndexed { i, b ->
                if (i > 0) Hairline()
                Row(Modifier.fillMaxWidth().heightIn(min = 56.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f).clickable { save(list.toMutableList().also { it[i] = b.copy(afterPr = !b.afterPr) }) }) {
                        Text(b.label, style = Type.title, color = palette.ink)
                        Text("${b.skill}${if (b.afterPr) " · after a PR" else ""}", style = Type.mono(12f), color = palette.mut)
                    }
                    if (i == main) Text("Main", Modifier.card(6.dp, palette.fill).padding(horizontal = 7.dp, vertical = 3.dp), style = Type.caption, color = palette.ink)
                    Icon(Icons.Filled.ArrowUpward, "Move up", Modifier.clickable(enabled = i > 0) { save(list.toMutableList().also { it.add(i - 1, it.removeAt(i)) }) }.padding(8.dp), tint = if (i > 0) palette.sub else palette.line)
                    Icon(Icons.Filled.ArrowDownward, "Move down", Modifier.clickable(enabled = i < list.lastIndex) { save(list.toMutableList().also { it.add(i + 1, it.removeAt(i)) }) }.padding(8.dp), tint = if (i < list.lastIndex) palette.sub else palette.line)
                    Icon(Icons.Outlined.RemoveCircleOutline, "Remove ${b.label}", Modifier.clickable { save(list - b) }.padding(8.dp), tint = palette.bad)
                }
            }
            SectionHead("Add from your agents' skills")
            val addable = skills.filter { s -> list.none { it.skill == s.name } }
            if (addable.isEmpty()) Note("No running agent has reported more skills.")
            addable.forEachIndexed { i, s ->
                if (i > 0) Hairline()
                Row(
                    Modifier.fillMaxWidth().clickable { save(list + ShipButton(s.name, ShipButtons.label(s.name), ShipButtons.afterPrByDefault(s.name))) }
                        .heightIn(min = 56.dp).padding(vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(s.name, style = Type.mono(14f, FontWeight.Medium), color = palette.ink)
                        Text("${s.description} · ${s.source}", style = Type.meta, color = palette.mut, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    Icon(Icons.Outlined.AddCircleOutline, "Add", tint = palette.ac)
                }
            }
            Note(
                "Listed from what your agents report, so another owner sees their own skills here. Kept on the hub, so every paired phone shows the same buttons. Tap a button's name to show it only after a PR.",
                Modifier.padding(top = 14.dp),
            )
        }
    }
}
