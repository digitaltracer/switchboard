package com.digitaltracer.switchboard

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.core.agentTarget
import com.digitaltracer.switchboard.core.displayName
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay

/**
 * A Herdr agent Switchboard didn't start: its output, refreshed while on screen. It has no
 * permission hooks, so its dialogs are answered in Herdr.
 */
@Composable
fun AgentScreen(model: AppModel, voice: Voice, listen: Listen, machine: String, name: String) {
    var output by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var showingOutput by remember { mutableStateOf(false) }
    val info = model.agents.firstOrNull { it.machineId == machine && it.name == name }
    val label = info?.displayName ?: name

    LaunchedEffect(machine, name, model.wakes) {
        while (true) {
            val hub = model.hub ?: break
            try {
                output = hub.agentOutput(machine, name, lines = 300).text
                error = null
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = e.message
            }
            delay(3_000)
        }
    }

    val target = agentTarget(machine, name, label)
    Page(model.backLabel, model::pop, bottomBar = {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Spacer(Modifier.weight(1f))
            MicButton(voice, 60.dp, "Talk to $label", "agentMic") { listen(target, false) }
            Row(Modifier.weight(1f), horizontalArrangement = Arrangement.Center) {
                BarIcon(Icons.Outlined.Keyboard, "Type a message to $label") { voice.type(target) }
            }
        }
    }) {
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, bottom = 40.dp),
            verticalArrangement = Arrangement.spacedBy(18.dp),
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                LargeTitle(label, horizontal = 0)
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    info?.let { StatusPill(SessionStatus.agent(it.agentStatus)) }
                    Text(listOfNotNull(info?.kind?.capitalized(), machine).joinToString(" · "), style = Type.subheadline, color = palette.secondary)
                }
            }
            Text(
                "Started outside Switchboard, so it has no approvals here. Answer its dialogs in Herdr.",
                style = Type.footnote, color = palette.secondary,
            )
            error?.let { Text(it, style = Type.footnote, color = palette.red) }
            model.said[AppModel.saidKey(agentTarget(machine, name, label))]?.let { SaidBubble(it) }
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("LATEST", style = Type.footnote, fontWeight = FontWeight.SemiBold, color = palette.secondary)
                OutputBlock(output, tag = "agentOutput")
                Row(Modifier.clickable { showingOutput = true }.padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("Show the full output", style = Type.body, color = palette.accent)
                    Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, null, Modifier.size(18.dp), tint = palette.accent)
                }
            }
        }
    }
    if (showingOutput) FullOutput(label, output) { showingOutput = false }
}
