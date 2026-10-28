package com.digitaltracer.switchboard

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.core.agentTarget
import com.digitaltracer.switchboard.core.displayName

/**
 * A Herdr agent Switchboard didn't start: its latest turns, refreshed while on screen. It has no
 * permission hooks, so its dialogs are answered in Herdr.
 */
@Composable
fun AgentScreen(model: AppModel, voice: Voice, listen: Listen, machine: String, name: String) {
    val convo = rememberConversation(ConversationSource.Agent(machine, name))
    val info = model.agents.firstOrNull { it.machineId == machine && it.name == name }
    val label = info?.displayName ?: name

    LaunchedEffect(machine, name, model.wakes) { convo.follow(model) }

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
                PageTitle(label)
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    info?.let { StatusPill(SessionStatus.agent(it.agentStatus)) }
                    Text(listOfNotNull(info?.kind?.capitalized(), machine).joinToString(" · "), style = Type.subheadline, color = palette.secondary)
                }
            }
            info?.title?.takeIf { it.isNotEmpty() }?.let { Text(it, style = Type.subheadline, fontWeight = FontWeight.SemiBold, color = palette.label) }
            model.said[AppModel.saidKey(agentTarget(machine, name, label))]?.let { SaidBubble(it) }
            SummaryCard(convo) { model.path.add(Route.Conversation(ConversationSource.Agent(machine, name), label)) }
            ConversationLinks(model, ConversationSource.Agent(machine, name), label, hasConversation = convo.unavailable == null)
            Text(
                "This agent asks for approval in Herdr, on its machine, not here.",
                Modifier.padding(horizontal = 4.dp), style = Type.footnote, color = palette.secondary,
            )
        }
    }
}
