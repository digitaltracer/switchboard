package com.digitaltracer.switchboard

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.api.models.Device
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/**
 * Paired devices, appearance and the hub. Revoking a device refuses its next request; to replace
 * a lost phone, pair the new one first, then revoke the old one here.
 */
@Composable
fun SettingsScreen(model: AppModel) {
    var devices by remember { mutableStateOf<List<Device>>(emptyList()) }
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

    GroupedList(model, "Settings") {
        error?.let { item { Text(it, style = Type.body, color = palette.red) } }
        item {
            InsetGroup("Paired devices") {
                devices.forEachIndexed { i, d ->
                    GroupRow(i == 0) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Text(d.name, style = Type.headline, color = palette.label)
                            if (d.thisDevice) Text("this phone", style = Type.caption, color = palette.secondary)
                            if (d.state != "active") Text(d.state, style = Type.caption, color = palette.needs)
                            Row(Modifier.weight(1f), horizontalArrangement = Arrangement.End) {
                                Text("Revoke", Modifier.clickable { revoking = d }.padding(4.dp), style = Type.subheadline, color = palette.red)
                            }
                        }
                        Text(d.fingerprint, style = Type.caption, fontFamily = FontFamily.Monospace, color = palette.secondary)
                        if (!d.pushEnabled) Text("No push", style = Type.caption, color = palette.secondary)
                    }
                }
            }
        }
        item {
            InsetGroup("Appearance") {
                AppModel.Appearance.entries.forEachIndexed { i, a ->
                    Column(Modifier.fillMaxWidth().clickable { model.chooseAppearance(a) }.testTag("appearance-${a.name}")) {
                        GroupRow(i == 0) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(a.label, Modifier.weight(1f), style = Type.body, color = palette.label)
                                if (model.appearance == a) Icon(Icons.Filled.Check, "Selected", Modifier.size(20.dp), tint = palette.accent)
                            }
                        }
                    }
                }
            }
        }
        model.pairing?.let { p ->
            item {
                InsetGroup("Hub") {
                    GroupRow(true) { Labeled(if (p.relay == null) "Address" else "Relay", p.hubUrl) }
                    model.meta?.let { m -> GroupRow(false) { Labeled("Oldest supported build", "${m.minClientBuild}") } }
                    Column(Modifier.fillMaxWidth().clickable { confirmForget = true }) {
                        GroupRow(false) { Text("Forget this hub", style = Type.body, color = palette.red) }
                    }
                }
            }
        }
    }

    revoking?.let { d ->
        AlertDialog(
            onDismissRequest = { revoking = null },
            text = { Text("Revoke ${d.name}? Its next request is refused.") },
            confirmButton = {
                TextButton({
                    revoking = null
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
                }) { Text("Revoke", color = palette.red) }
            },
            dismissButton = { TextButton({ revoking = null }) { Text("Cancel", color = palette.accent) } },
        )
    }
    if (confirmForget) {
        AlertDialog(
            onDismissRequest = { confirmForget = false },
            text = { Text("Forget this hub? This phone's keys are deleted; revoke it at the hub too.") },
            confirmButton = {
                TextButton({
                    confirmForget = false
                    model.forget()
                }) { Text("Forget", color = palette.red) }
            },
            dismissButton = { TextButton({ confirmForget = false }) { Text("Cancel", color = palette.accent) } },
        )
    }
}
