package com.digitaltracer.switchboard

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Activity
import com.digitaltracer.switchboard.api.models.AwayAgentView
import com.digitaltracer.switchboard.api.models.AwaySettingsView
import com.digitaltracer.switchboard.core.AwayDigest
import com.digitaltracer.switchboard.core.displayName
import java.text.DateFormat
import java.util.Date
import kotlinx.coroutines.launch

/**
 * The Inbox card "While you were away": each agent that did something since the phone last had
 * the app open, needs-you first, until "Mark as seen". An opt-in summary replaces the rows.
 */
@Composable
fun AwayCard(model: AppModel, activity: Activity) {
    Column(Modifier.fillMaxWidth().padding(bottom = 14.dp).card(18.dp, palette.sf).border(1.dp, palette.line, RoundedCornerShape(18.dp)).testTag("awayCard")) {
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("While you were away", Modifier.weight(1f), style = Type.title.copy(fontWeight = FontWeight.SemiBold), color = palette.ink)
            Text(AwayDigest.duration(activity.now - activity.since), style = Type.meta, color = palette.mut)
        }
        val summary = model.awaySummary
        if (summary != null) {
            Text(summary, Modifier.padding(horizontal = 16.dp).padding(bottom = 14.dp).testTag("awaySummary"), style = Type.text, color = palette.ink)
        } else {
            activity.agents.forEach { a -> AwayRow(model, a, activity.now) }
        }
        Hairline()
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                if (summary == null) "Since you last opened the app" else "Summarised by Claude, from the hub's facts",
                Modifier.weight(1f), style = Type.meta, color = palette.mut,
            )
            Text("Mark as seen", Modifier.clickable { model.markAwaySeen() }.testTag("markAwaySeen"), style = Type.meta.copy(fontWeight = FontWeight.Medium), color = palette.ac)
        }
    }
}

@Composable
private fun AwayRow(model: AppModel, a: AwayAgentView, now: Long) {
    val tone = AwayDigest.tone(a)
    val colour = when (tone) {
        "need" -> palette.warn
        "bad" -> palette.bad
        "working" -> palette.ac
        else -> palette.ok
    }
    val title = model.sessions.firstOrNull { it.id == a.sessionId }?.label
        ?: model.agents.firstOrNull { it.machineId == a.machineId && it.name == a.agent }?.displayName ?: a.agent
    Hairline()
    Row(
        Modifier.fillMaxWidth().background(if (tone == "need") palette.wbg else androidx.compose.ui.graphics.Color.Transparent)
            .clickable { model.path.add(a.sessionId?.let { Route.SessionPage(it) } ?: Route.AgentPage(a.machineId, a.agent)) }
            .padding(horizontal = 16.dp, vertical = 10.dp),
    ) {
        Dot(colour)
        Spacer(Modifier.width(12.dp))
        Column {
            Text(title, style = Type.text.copy(fontWeight = FontWeight.SemiBold), color = palette.ink, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(AwayDigest.line(a, now), style = Type.sub, color = palette.sub, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** An agent page's "Since you looked": the facts in order, then its last reply. */
@Composable
fun AwayTimeline(agent: AwayAgentView, since: Long, now: Long, reply: String? = null) {
    val time = DateFormat.getTimeInstance(DateFormat.SHORT)
    Column(Modifier.testTag("sinceYouLooked")) {
        SectionHead("Since you looked · ${AwayDigest.duration(now - since)}")
        Column(Modifier.fillMaxWidth().card(16.dp, palette.sf).border(1.dp, palette.line, RoundedCornerShape(16.dp)).padding(horizontal = 16.dp)) {
            val rows = AwayDigest.timeline(agent.facts)
            rows.forEachIndexed { i, f ->
                if (i > 0) Hairline()
                Row(Modifier.fillMaxWidth().padding(vertical = 11.dp)) {
                    Text(time.format(Date(f.at)), Modifier.width(64.dp), style = Type.mono(12f), color = palette.mut)
                    Column(Modifier.padding(start = 10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text(
                            f.text,
                            style = Type.text.copy(fontWeight = if (f.kind == "finished") FontWeight.Medium else FontWeight.Normal),
                            color = when (f.kind) { "finished" -> palette.ok; "error" -> palette.bad; else -> palette.ink },
                        )
                        if (f.kind == "finished" && i == rows.indexOfLast { it.kind == "finished" }) {
                            (reply ?: agent.lastReply)?.let { r ->
                                // A reply's markdown links read as their text here.
                                val plain = r.replace(Regex("""\[([^\]]+)]\([^)]*\)"""), "$1").replace("**", "")
                                Text("\"$plain\"", style = Type.text, color = palette.sub)
                            }
                        }
                    }
                }
            }
        }
        Note("Facts the hub recorded, plus the agent's own last reply. No summarising model.", Modifier.padding(top = 8.dp))
    }
}

/** Settings › While you were away: the opt-in summary and its model, kept on the hub. */
@Composable
fun AwaySettingsSection(model: AppModel) {
    val scope = rememberCoroutineScope()
    var settings by remember { mutableStateOf<AwaySettingsView?>(null) }
    var name by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        runCatching { model.hub?.awaySettings() }.getOrNull()?.let { settings = it; name = it.model }
    }
    fun save(summary: Boolean) {
        val next = AwaySettingsView(summary, name.ifBlank { "haiku" })
        scope.launch {
            val hub = model.hub ?: return@launch
            runCatching { hub.setAwaySettings(next) }.onSuccess { settings = next; error = null }.onFailure { error = it.message }
        }
    }
    SectionHead("While you were away")
    Row(Modifier.fillMaxWidth().heightIn(min = 50.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("Summarise with Claude", Modifier.weight(1f), style = Type.field, color = palette.ink)
        Switch(
            settings?.summary == true, { save(it) }, Modifier.testTag("awaySummaryToggle"), enabled = settings != null,
            colors = SwitchDefaults.colors(checkedTrackColor = palette.ink),
        )
    }
    Note(
        "Off: you see the facts your hub recorded. On: when this phone comes back, your machine runs claude -p on your own Claude account, with every tool, hook, plugin and MCP server off, to summarise them. Masked first, like everything that leaves the machine.",
    )
    if (settings?.summary == true) {
        Row(Modifier.fillMaxWidth().heightIn(min = 44.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Model", Modifier.weight(1f), style = Type.field, color = palette.sub)
            Box(Modifier.width(180.dp)) {
                BasicTextField(name, { name = it }, textStyle = Type.mono(14f).copy(color = palette.ink), singleLine = true, cursorBrush = SolidColor(palette.ac))
            }
            Text("Save", Modifier.clickable { save(true) }.padding(8.dp), style = Type.meta, color = palette.ac)
        }
    }
    error?.let { Note(it, color = palette.bad) }
}
