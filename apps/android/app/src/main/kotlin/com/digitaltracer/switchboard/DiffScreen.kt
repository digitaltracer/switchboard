package com.digitaltracer.switchboard

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowUp
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.DiffHunkView
import com.digitaltracer.switchboard.api.models.DiffLineView
import com.digitaltracer.switchboard.api.models.FileDiffView
import com.digitaltracer.switchboard.core.DiffFolding
import com.digitaltracer.switchboard.core.DiffQuote
import com.digitaltracer.switchboard.core.DiffRow
import com.digitaltracer.switchboard.core.fileSize
import com.digitaltracer.switchboard.core.sessionTarget
import java.util.UUID
import kotlinx.coroutines.launch

/**
 * One changed file's unified diff, read-only and masked. Long unchanged runs fold; a big file
 * loads a page of hunks at a time. Tapping a line number, then another, selects lines to ask
 * the agent about.
 */
@Composable
fun DiffScreen(model: AppModel, voice: Voice, route: Route.Diff) {
    var index by remember(route) { mutableStateOf(route.index) }
    var diff by remember(route) { mutableStateOf<FileDiffView?>(null) }
    var error by remember(route) { mutableStateOf<String?>(null) }
    var loadingMore by remember { mutableStateOf(false) }
    var wrap by remember { mutableStateOf(true) }
    val open = remember(route, index) { mutableStateMapOf<Int, Set<Int>>() }
    // (hunk, anchor line, end line)
    var selection by remember(route, index) { mutableStateOf<Triple<Int, Int, Int>?>(null) }
    var asking by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val path = route.paths[index]

    LaunchedEffect(route, index) {
        diff = null
        error = null
        val hub = model.hub ?: return@LaunchedEffect
        runCatching { hub.diff(route.sessionId, path) }.onSuccess { diff = it }.onFailure { error = it.message }
    }

    fun quote(): List<Pair<Int, String>> {
        val (h, a, e) = selection ?: return emptyList()
        val hunk = diff?.hunks?.getOrNull(h) ?: return emptyList()
        return (minOf(a, e)..maxOf(a, e)).mapNotNull { i -> hunk.lines.getOrNull(i)?.let { (it.new ?: it.old ?: 0) to it.text } }
    }

    fun tap(h: Int, i: Int) {
        val s = selection
        selection = when {
            s != null && s.first == h && s.second == s.third && s.second != i -> Triple(h, s.second, i)
            s != null && s.first == h && s.second == i && s.third == i -> null
            else -> Triple(h, i, i)
        }
    }

    InboxPage(
        top = {
            PageTop("Back to Changes", model::pop) {
                Text("Changes", Modifier.clickable(onClick = model::pop), style = Type.field.copy(fontWeight = FontWeight.Medium), color = palette.ac)
                Spacer(Modifier.weight(1f))
                Text("${index + 1} of ${route.paths.size}", style = Type.meta, color = palette.mut)
                Spacer(Modifier.width(10.dp))
                Icon(
                    Icons.Filled.KeyboardArrowUp, "Previous file",
                    Modifier.clickable(enabled = index > 0) { index-- }.padding(6.dp),
                    tint = if (index > 0) palette.ink else palette.mut,
                )
                Icon(
                    Icons.Filled.KeyboardArrowDown, "Next file",
                    Modifier.clickable(enabled = index + 1 < route.paths.size) { index++ }.padding(6.dp),
                    tint = if (index + 1 < route.paths.size) palette.ink else palette.mut,
                )
            }
        },
        bottom = if (selection != null) {
            {
                val q = quote()
                val lo = q.minOfOrNull { it.first } ?: 0
                val hi = q.maxOfOrNull { it.first } ?: 0
                Row(
                    Modifier.fillMaxWidth().background(palette.sf).padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(if (lo == hi) "Line $lo selected" else "Lines $lo–$hi selected", style = Type.title, color = palette.ink)
                        Text("Tap a line number, then another, to select", style = Type.caption, color = palette.mut)
                    }
                    BigButton("Ask agent", Modifier.testTag("askAgent"), shape = RoundedCornerShape(12.dp)) { asking = true }
                }
            }
        } else null,
    ) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) {
            Column(Modifier.padding(horizontal = 20.dp).padding(top = 8.dp, bottom = 12.dp)) {
                Text(path, style = Type.mono(14f, FontWeight.Medium), color = palette.ink)
                val d = diff
                if (d != null && !d.binary && !d.withheld) {
                    Row(Modifier.padding(top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text("+${d.added ?: 0} ", style = Type.mono(12.5f), color = palette.ok)
                        Text("−${d.deleted ?: 0}", style = Type.mono(12.5f), color = palette.bad)
                        Text(" · ${d.hunkCount} ${if (d.hunkCount == 1) "hunk" else "hunks"}", style = Type.mono(12.5f), color = palette.mut)
                        Spacer(Modifier.weight(1f))
                        Text(
                            if (wrap) "Wrap ✓" else "Wrap", Modifier.clickable { wrap = !wrap }.padding(6.dp),
                            style = Type.meta, color = palette.ac,
                        )
                    }
                }
            }
            val d = diff
            when {
                d == null && error != null -> Note(error!!, Modifier.padding(20.dp), palette.bad)
                d == null -> StateRow("Loading…", busy = true)
                d.withheld -> Note("This file is the kind that holds secrets, such as keys or an env file, so it isn't shown on the phone.", Modifier.padding(20.dp))
                d.binary -> Note("Binary file${d.propertySize?.let { " · ${fileSize(it)}" } ?: ""}. Only its size is shown.", Modifier.padding(20.dp))
                d.hunks.isEmpty() -> Note("No changes to show.", Modifier.padding(20.dp))
                else -> {
                    d.hunks.forEachIndexed { h, hunk ->
                        Hunk(h, hunk, wrap, open[h] ?: emptySet(), selection, { from -> open[h] = (open[h] ?: emptySet()) + from }, ::tap)
                    }
                    if (d.next != null) {
                        Text(
                            if (loadingMore) "Loading…" else "Show more of this file",
                            Modifier.clickable(enabled = !loadingMore) {
                                scope.launch {
                                    val hub = model.hub ?: return@launch
                                    loadingMore = true
                                    runCatching { hub.diff(route.sessionId, path, d.next!!) }
                                        .onSuccess { page -> diff = d.copy(hunks = d.hunks + page.hunks, next = page.next) }
                                        .onFailure { error = it.message }
                                    loadingMore = false
                                }
                            }.padding(20.dp),
                            style = Type.title, color = palette.ac,
                        )
                    }
                }
            }
            if (d != null) Note("Secrets are masked before they leave your machine, as in all output.", Modifier.padding(horizontal = 20.dp, vertical = 16.dp))
        }
    }
    if (asking) AskSheet(model, voice, route.sessionId, path, quote(), { asking = false }) { asking = false; selection = null }
}

@Composable
private fun Hunk(
    h: Int,
    hunk: DiffHunkView,
    wrap: Boolean,
    open: Set<Int>,
    selection: Triple<Int, Int, Int>?,
    unfold: (Int) -> Unit,
    tap: (Int, Int) -> Unit,
) {
    Text(
        hunk.header, Modifier.fillMaxWidth().background(palette.fill).padding(horizontal = 12.dp, vertical = 8.dp),
        style = Type.mono(11.5f), color = palette.mut, maxLines = 1, overflow = TextOverflow.Ellipsis,
    )
    val body = @Composable {
        Column {
            DiffFolding.rows(hunk.lines.map { it.kind }, open).forEach { row ->
                when (row) {
                    is DiffRow.Line -> {
                        val selected = selection != null && selection.first == h &&
                            row.index in minOf(selection.second, selection.third)..maxOf(selection.second, selection.third)
                        Line(hunk.lines[row.index], wrap, selected) { tap(h, row.index) }
                    }
                    is DiffRow.Fold -> Text(
                        "Show ${row.count} unchanged lines",
                        Modifier.fillMaxWidth().clickable { unfold(row.from) }.padding(horizontal = 12.dp, vertical = 9.dp),
                        style = Type.meta.copy(fontWeight = FontWeight.Medium), color = palette.ac,
                    )
                }
            }
            if (hunk.cut) Note("This change is too long to show in full here.", Modifier.padding(12.dp))
        }
    }
    if (wrap) body() else Row(Modifier.horizontalScroll(rememberScrollState())) { body() }
}

@Composable
private fun Line(l: DiffLineView, wrap: Boolean, selected: Boolean, tap: () -> Unit) {
    val bg = when (l.kind) {
        "+" -> palette.ok.copy(alpha = 0.12f)
        "-" -> palette.bad.copy(alpha = 0.12f)
        else -> androidx.compose.ui.graphics.Color.Transparent
    }
    Row(
        Modifier.then(if (wrap) Modifier.fillMaxWidth() else Modifier).background(bg)
            .then(if (selected) Modifier.border(1.5.dp, palette.ac) else Modifier).padding(vertical = 2.dp).padding(end = 12.dp),
    ) {
        Row(Modifier.clickable(onClick = tap).semantics { contentDescription = "Line ${l.new ?: l.old ?: 0}" }) {
            val numColor = if (selected) palette.ac else if (l.kind == "-") palette.bad else palette.mut
            Text(l.old?.toString() ?: "", Modifier.width(34.dp), style = Type.mono(11f), color = numColor, textAlign = androidx.compose.ui.text.style.TextAlign.End)
            Text(l.new?.toString() ?: "", Modifier.width(34.dp), style = Type.mono(11f), color = numColor, textAlign = androidx.compose.ui.text.style.TextAlign.End)
        }
        Text(if (l.kind == " ") " " else l.kind, Modifier.width(16.dp), style = Type.mono(12.5f), color = palette.mut, textAlign = androidx.compose.ui.text.style.TextAlign.Center)
        Text(l.text.ifEmpty { " " }, style = Type.mono(12.5f), color = palette.ink, softWrap = wrap)
    }
}

/** Asks the agent about the selected lines with an ordinary message. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AskSheet(model: AppModel, voice: Voice, sessionId: String, path: String, lines: List<Pair<Int, String>>, cancel: () -> Unit, sent: () -> Unit) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    var question by remember { mutableStateOf("") }
    // The × takes the quoted lines out; then only the question goes.
    var quoted by remember { mutableStateOf(true) }
    val askMic = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) voice.dictate(question) else voice.denied()
    }
    LaunchedEffect(voice.phase, voice.retrying, voice.canRetry) {
        if (voice.phase == Voice.Phase.Review && voice.dictating && !voice.retrying && !voice.canRetry) {
            voice.text.trim().takeIf { it.isNotEmpty() }?.let { question = it }
            voice.finish()
        }
    }
    var sending by remember { mutableStateOf(false) }
    var note by remember { mutableStateOf<String?>(null) }
    val requestId = remember { UUID.randomUUID().toString() }
    val session = model.sessions.firstOrNull { it.id == sessionId }
    val agent = session?.label ?: "the agent"
    ModalBottomSheet(
        onDismissRequest = { if (!voice.dictating) cancel() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = palette.bg,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
    ) {
        Box {
        Column(Modifier.fillMaxHeight(0.95f).imePadding().navigationBarsPadding().padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Row(Modifier.fillMaxWidth().height(44.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Cancel", Modifier.clickable(onClick = cancel).padding(vertical = 10.dp), style = Type.field.copy(fontWeight = FontWeight.Medium), color = palette.ac)
                Spacer(Modifier.weight(1f))
                Text("To $agent", style = Type.meta, color = palette.mut, maxLines = 1)
            }
            if (quoted) Column(Modifier.fillMaxWidth().card(12.dp, palette.sf).border(1.dp, palette.line, RoundedCornerShape(12.dp))) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(DiffQuote.location(path, lines.map { it.first }), Modifier.weight(1f).padding(12.dp), style = Type.mono(12.5f), color = palette.ink, maxLines = 1, overflow = TextOverflow.StartEllipsis)
                    Icon(Icons.Filled.Close, "Remove the quoted lines", Modifier.clickable { quoted = false }.padding(10.dp), tint = palette.mut)
                }
                Hairline()
                Text(lines.joinToString("\n") { it.second }, Modifier.padding(12.dp), style = Type.mono(12f), color = palette.ink, maxLines = 8)
            }
            SectionHead("Your message", top = 4.dp)
            BasicTextField(
                question, { question = it },
                Modifier.fillMaxWidth().heightIn(min = 140.dp).card(12.dp, palette.sf).border(1.dp, palette.line, RoundedCornerShape(12.dp)).padding(12.dp).testTag("askQuestion"),
                textStyle = Type.draft.copy(color = palette.ink, fontSize = Type.title3.fontSize),
                cursorBrush = SolidColor(palette.ac),
            )
            Note(
                note ?: if (quoted) "Sent as an ordinary message. The agent gets the file, the line numbers and these lines, then answers in Conversation."
                else "Sent as an ordinary message, without the lines.",
                color = if (note == null) null else palette.bad,
            )
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            MicDisc(52.dp, "Dictate the message", "askDictate") {
                if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) voice.dictate(question)
                else askMic.launch(Manifest.permission.RECORD_AUDIO)
            }
            BigButton(
                if (sending) "Sending…" else "Send to $agent", Modifier.weight(1f).testTag("askSend"),
                enabled = !sending && !voice.dictating && question.isNotBlank() && session != null, busy = sending,
            ) {
                val s = session ?: return@BigButton
                scope.launch {
                    val hub = model.hub ?: return@launch
                    sending = true
                    val text = if (quoted) DiffQuote.message(path, lines, question) else question.trim()
                    runCatching { hub.message(text, requestId, null, target = sessionTarget(s)) }
                        .onSuccess { r ->
                            if (r.outcome == "sent" || r.outcome == "started") {
                                model.record(text, r.current, started = false)
                                sent()
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
        if (voice.dictating) VoiceOverlay(voice)
        }
    }
}
