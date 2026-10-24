package com.digitaltracer.switchboard

import android.view.HapticFeedbackConstants
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.outlined.MicOff
import androidx.compose.material.icons.outlined.MonitorHeart
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
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
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.MessagePreview
import com.digitaltracer.switchboard.core.HubError
import com.digitaltracer.switchboard.core.RouteOption
import com.digitaltracer.switchboard.core.RouteOptions
import com.digitaltracer.switchboard.core.StartFlow
import com.digitaltracer.switchboard.core.StartState
import com.digitaltracer.switchboard.core.sessionTarget
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Review before sending: the words, heard or typed, and where they'll go. The hub's preview
 * picks the destination; the user can pick another.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SendSheet(model: AppModel, voice: Voice, listen: Listen) {
    var preview by remember { mutableStateOf<MessagePreview?>(null) }
    var routes by remember { mutableStateOf(RouteOptions.Result(emptyList(), null)) }
    var picked by remember { mutableStateOf<String?>(null) }
    var note by remember { mutableStateOf<String?>(null) }
    var sending by remember { mutableStateOf(false) }
    // One per draft and destination, so a retried send is never typed twice.
    var requestId by remember { mutableStateOf(UUID.randomUUID().toString()) }
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    val focus = remember { FocusRequester() }
    val current = voice.target ?: model.current
    val chosen = routes.options.firstOrNull { it.id == (picked ?: routes.suggested) }
    val empty = voice.text.isBlank()

    // Asks the hub where the text would go, after a short pause in typing.
    LaunchedEffect(voice.text) {
        requestId = UUID.randomUUID().toString()
        val text = voice.text.trim()
        val hub = model.hub
        if (text.isEmpty() || hub == null) {
            preview = null
            routes = RouteOptions.Result(emptyList(), null)
            return@LaunchedEffect
        }
        delay(300)
        try {
            val p = hub.preview(text, current)
            preview = p
            routes = RouteOptions.build(p, model.sessions, voice.target)
            if (picked != null && routes.options.none { it.id == picked }) picked = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            note = e.message
        }
    }
    LaunchedEffect(Unit) { if (voice.text.isEmpty()) focus.requestFocus() }

    // A lost answer: the hub may have acted, so look the request up before calling it failed.
    suspend fun reconcile(text: String) {
        val hub = model.hub ?: return
        note = "Not sure that got through. Checking…"
        var state: StartState = StartState.Unknown(requestId)
        for (attempt in 0 until 30) {
            state = StartFlow.reconcile(hub, requestId, attempt)
            if (state !is StartState.Unknown) break
            delay(2_000)
        }
        when (val s = state) {
            is StartState.Started -> {
                runCatching { hub.session(s.sessionId) }.onSuccess {
                    model.current = sessionTarget(it)
                    model.record(text, sessionTarget(it), started = true)
                }
                voice.finish()
            }
            is StartState.Unknown -> note = "Still not sure it went through. Sending it again is safe."
            is StartState.Failed -> note = "That didn't reach the hub. Send it again."
        }
    }

    fun send() {
        val hub = model.hub ?: return
        val option = chosen ?: return
        val text = voice.text.trim()
        scope.launch {
            sending = true
            try {
                val r = when (val a = option.action) {
                    RouteOption.Action.AsPreviewed -> hub.message(text, requestId, current)
                    is RouteOption.Action.Start -> hub.message(text, requestId, current, projectId = a.projectId)
                    is RouteOption.Action.Send -> hub.message(text, requestId, current, target = a.target)
                }
                note = r.message
                when (r.outcome) {
                    "started", "sent", "selected" -> {
                        model.current = r.current
                        model.record(text, r.current, started = r.outcome == "started")
                        view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
                        voice.finish()
                    }
                    "choose_project", "choose_target" -> {
                        // The hub still couldn't tell; its options are the destinations now.
                        val asked = MessagePreview(r.outcome, r.message, r.options)
                        preview = asked
                        routes = RouteOptions.build(asked, model.sessions, voice.target)
                        picked = null
                    }
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: HubError) {
                if (e.outcomeUnknown) reconcile(text) else note = e.message
            } catch (e: Exception) {
                note = e.message
            } finally {
                sending = false
            }
        }
    }

    ModalBottomSheet(
        onDismissRequest = { voice.finish() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = palette.background,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
    ) {
        Column(Modifier.imePadding().navigationBarsPadding()) {
            Box(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 12.dp)) {
                Text("Cancel", Modifier.align(Alignment.CenterStart).clickable { voice.finish() }, style = Type.body, color = palette.accent)
                Text("Ready to send", Modifier.align(Alignment.Center), style = Type.headline, color = palette.label)
            }
            Column(
                Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                voice.error?.let {
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.Outlined.MicOff, null, Modifier.size(16.dp), tint = palette.secondary)
                        Text(it, style = Type.footnote, color = palette.secondary)
                    }
                }
                val fieldStyle = Type.title3.copy(fontWeight = FontWeight.Normal, color = palette.label)
                BasicTextField(
                    voice.text, { voice.text = it },
                    Modifier.fillMaxWidth().card(16.dp, palette.card).padding(14.dp).heightIn(min = 56.dp)
                        .focusRequester(focus).testTag("composeField"),
                    textStyle = fieldStyle,
                    cursorBrush = SolidColor(palette.accent),
                    minLines = 2, maxLines = 6,
                    decorationBox = { inner ->
                        if (voice.text.isEmpty()) {
                            Text(current?.let { "Message ${it.label}" } ?: "Say or type what you want done", style = fieldStyle.copy(color = palette.tertiary))
                        }
                        inner()
                    },
                )
                if (routes.options.isNotEmpty()) {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(
                            preview?.message ?: "Goes to", Modifier.padding(horizontal = 4.dp).testTag("previewMessage"),
                            style = Type.footnote, fontWeight = FontWeight.SemiBold, color = palette.secondary,
                        )
                        Column(Modifier.fillMaxWidth().card(14.dp, palette.card)) {
                            routes.options.forEachIndexed { i, o ->
                                RouteRow(o, o.id == chosen?.id, i == 0) {
                                    picked = o.id
                                    requestId = UUID.randomUUID().toString()
                                }
                            }
                        }
                    }
                } else if (preview?.outcome == "refused") {
                    Text(preview?.message.orEmpty(), style = Type.footnote, color = palette.secondary)
                }
                note?.let { Text(it, Modifier.testTag("composeNote"), style = Type.footnote, color = palette.secondary) }
            }
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(
                    Modifier.size(50.dp).background(palette.fill, CircleShape).clickable { listen(voice.target, keepText = true) },
                    Alignment.Center,
                ) { Icon(Icons.Filled.Mic, "Keep talking", Modifier.size(22.dp), tint = palette.accent) }
                val ready = chosen != null && !empty && !sending
                Box(
                    Modifier.weight(1f).height(50.dp).background(if (chosen == null || empty) Color.Gray else palette.accent, CircleShape)
                        .clickable(enabled = ready) { send() }.testTag("composeSend"),
                    Alignment.Center,
                ) {
                    if (sending) {
                        CircularProgressIndicator(Modifier.size(22.dp), color = Color.White, strokeWidth = 2.dp)
                    } else {
                        Text(
                            chosen?.verb ?: if (empty) "Say or type something" else "Pick where this goes",
                            style = Type.headline, color = Color.White, maxLines = 1,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun RouteRow(option: RouteOption, selected: Boolean, first: Boolean, pick: () -> Unit) {
    Column(Modifier.fillMaxWidth().background(if (selected) palette.accent.copy(alpha = 0.06f) else Color.Transparent).clickable(onClick = pick)
        .testTag("route-${option.id}")) {
        if (!first) HorizontalDivider(Modifier.padding(start = 58.dp), 0.5.dp, palette.separator)
        Row(Modifier.heightIn(min = 56.dp).padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(
                Modifier.size(32.dp).background(if (option.isNew) palette.fill else palette.accent.copy(alpha = 0.14f), RoundedCornerShape(9.dp)),
                Alignment.Center,
            ) {
                Icon(
                    if (option.isNew) Icons.Filled.Add else Icons.Outlined.MonitorHeart, null, Modifier.size(18.dp),
                    tint = if (option.isNew) palette.label else palette.accent,
                )
            }
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(option.title, style = Type.body, fontWeight = FontWeight.SemiBold, color = palette.label, maxLines = 1)
                Text(option.detail, style = Type.footnote, color = palette.secondary, maxLines = 1)
            }
            Icon(Icons.Filled.Check, null, Modifier.size(20.dp), tint = palette.accent.copy(alpha = if (selected) 1f else 0f))
        }
    }
}
