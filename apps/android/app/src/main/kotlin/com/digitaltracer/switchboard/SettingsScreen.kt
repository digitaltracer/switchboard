package com.digitaltracer.switchboard

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Device
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/**
 * Paired devices, appearance, voice and the hub. Revoking a device refuses its next request; to
 * replace a lost phone, pair the new one first, then revoke the old one here.
 */
@Composable
fun SettingsScreen(model: AppModel) {
    var devices by remember { mutableStateOf<List<Device>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var revoking by remember { mutableStateOf<Device?>(null) }
    var confirmForget by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    suspend fun load() {
        try {
            devices = model.hub?.devices().orEmpty()
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message
        }
    }
    LaunchedEffect(Unit) { load() }

    InboxPage(
        top = {
            Row(
                Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.statusBars).padding(start = 20.dp, end = 20.dp, top = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("Settings", Modifier.weight(1f), style = Type.h1, color = palette.ink)
                Text("Done", Modifier.clickable(onClick = model::pop).padding(vertical = 10.dp).testTag("back"), style = Type.field.copy(fontWeight = FontWeight.Medium), color = palette.ac)
            }
        },
    ) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, bottom = 32.dp)) {
            error?.let { Note(it, Modifier.padding(top = 12.dp), palette.bad) }
            SectionHead("Paired devices", devices?.size?.toString(), top = 16.dp)
            if (devices == null && error == null) StateRow("Listing devices…", busy = true)
            devices.orEmpty().forEachIndexed { i, d ->
                Column(Modifier.fillMaxWidth()) {
                    if (i > 0) Hairline()
                    Row(Modifier.fillMaxWidth().heightIn(min = 60.dp).padding(vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text(d.name, style = Type.title, color = palette.ink)
                                if (d.thisDevice) Chip("this phone")
                                if (d.state != "active") Text(d.state, style = Type.meta, color = palette.warn)
                            }
                            Text(d.fingerprint, style = Type.mono(12.5f), color = palette.sub, maxLines = 1)
                            if (!d.pushEnabled) Text("No push", style = Type.meta, color = palette.mut)
                        }
                        Text(
                            "Revoke", Modifier.clickable { revoking = d }.padding(vertical = 12.dp, horizontal = 4.dp).testTag("revoke-${d.name}"),
                            style = Type.text.copy(fontWeight = FontWeight.Medium), color = palette.bad,
                        )
                    }
                }
            }
            Hairline()
            SectionHead("Appearance")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                AppModel.Appearance.entries.forEach { a ->
                    val on = model.appearance == a
                    Box(
                        Modifier.weight(1f).height(44.dp).clip(RoundedCornerShape(12.dp)).background(if (on) palette.ink else palette.fill)
                            .clickable { model.chooseAppearance(a) }.semantics { selected = on }.testTag("appearance-${a.name}"),
                        Alignment.Center,
                    ) { Text(a.label, style = Type.text.copy(fontWeight = FontWeight.Medium), color = if (on) palette.inv else palette.sub) }
                }
            }
            VoiceKeySection(model.voiceKey)
            model.pairing?.let { p ->
                SectionHead("Hub")
                FieldRow(if (p.relay == null) "Address" else "Relay", runCatching { java.net.URI(p.hubUrl).host }.getOrNull() ?: p.hubUrl, mono = true, first = true)
                model.meta?.let { m -> FieldRow("Oldest supported build", "${m.minClientBuild}") }
                Column(Modifier.fillMaxWidth().clickable { confirmForget = true }.testTag("forgetHub")) {
                    Hairline()
                    Text("Forget this hub", Modifier.padding(vertical = 14.dp), style = Type.field.copy(fontWeight = FontWeight.Medium), color = palette.bad)
                }
                Hairline()
            }
        }
    }

    revoking?.let { d ->
        ChoiceSheet(
            "Revoke ${d.name}?",
            listOf(
                Triple<String, String?, () -> Unit>(
                    "Revoke",
                    if (d.thisDevice) "This phone's next request is refused and it forgets the hub. Pair it again to use it."
                    else "Its next request is refused. It stays paired until then, and can't be un-revoked.",
                ) {
                    scope.launch {
                        try {
                            model.hub?.revoke(d.id)
                            if (d.thisDevice) model.forget() else load()
                        } catch (e: CancellationException) {
                            throw e
                        } catch (e: Exception) {
                            error = e.message
                        }
                    }
                },
            ),
        ) { revoking = null }
    }
    if (confirmForget) {
        ChoiceSheet(
            "Forget this hub?",
            listOf(
                Triple<String, String?, () -> Unit>("Forget", "This phone's keys are deleted. Revoke it at the hub too.") { model.forget() },
            ),
        ) { confirmForget = false }
    }
}

/** An OpenAI key, kept on this phone. Once saved it's never shown again. */
@Composable
private fun VoiceKeySection(key: VoiceKey) {
    var entered by remember { mutableStateOf("") }
    var failed by remember { mutableStateOf(false) }
    SectionHead("Voice", "On this phone")
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        val box = Modifier.weight(1f).heightIn(min = 52.dp).clip(RoundedCornerShape(14.dp)).background(palette.sf)
            .border(1.dp, palette.line, RoundedCornerShape(14.dp)).padding(horizontal = 16.dp)
        if (key.saved) {
            Box(box.testTag("voiceKeySaved"), Alignment.CenterStart) { Text("Key saved", style = Type.field, color = palette.ink) }
            BigButton("Remove key", Modifier.widthIn(min = 130.dp).testTag("voiceKeyRemove"), ButtonKind.Outline, destructive = true) { key.remove() }
        } else {
            Box(box, Alignment.CenterStart) {
                BasicTextField(
                    entered, { entered = it },
                    Modifier.fillMaxWidth().testTag("voiceKeyField"),
                    textStyle = Type.field.copy(color = palette.ink),
                    cursorBrush = SolidColor(palette.ac),
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false, keyboardType = KeyboardType.Password),
                    decorationBox = { inner ->
                        if (entered.isEmpty()) Text("OpenAI API key", style = Type.field, color = palette.mut)
                        inner()
                    },
                )
            }
            BigButton("Save key", Modifier.widthIn(min = 108.dp).testTag("voiceKeySave"), enabled = entered.isNotBlank()) {
                failed = !key.save(entered)
                entered = ""
            }
        }
    }
    if (failed) Note("This phone couldn't store the key, so voice stays on the device.", Modifier.padding(top = 8.dp), palette.bad)
    Note(VoiceKey.DISCLOSURE, Modifier.padding(top = 8.dp))
}
