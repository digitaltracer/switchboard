package com.digitaltracer.switchboard

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.core.PairingLink
import com.google.mlkit.common.MlKitException
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning

/** Scan or paste the link from `switchboard admin pair-device`, then compare fingerprints. */
@Composable
fun PairingScreen(model: AppModel) {
    var linkText by remember { mutableStateOf("") }
    var name by remember { mutableStateOf(model.defaultName) }
    var cameraNote by remember { mutableStateOf<String?>(null) }
    val context = LocalContext.current
    val opened = model.openedLink
    LaunchedEffect(opened) {
        if (opened != null) {
            linkText = opened
            model.openedLink = null
        }
    }

    Page(back = null) {
        Column(
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(start = 16.dp, end = 16.dp, bottom = 40.dp),
            verticalArrangement = Arrangement.spacedBy(24.dp),
        ) {
            LargeTitle("Pair with your hub", horizontal = 0)
            when (val phase = model.phase) {
                is AppModel.Phase.Confirming -> InsetGroup("Confirm on your computer") {
                    GroupRow(true) {
                        Text(
                            "Check that the terminal running pair-device shows exactly this fingerprint, then type yes there.",
                            style = Type.body, color = palette.label,
                        )
                    }
                    GroupRow(false) {
                        Text(phase.fingerprint, Modifier.testTag("fingerprint"), style = Type.title3, fontFamily = FontFamily.Monospace, color = palette.label)
                    }
                    GroupRow(false) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            CircularProgressIndicator(Modifier.size(18.dp), color = palette.secondary, strokeWidth = 2.dp)
                            Text("Waiting for the hub…", style = Type.body, color = palette.secondary)
                        }
                    }
                    Column(Modifier.fillMaxWidth().clickable(onClick = model::cancelPairing).testTag("cancelPairing")) {
                        GroupRow(false) { Text("Cancel", style = Type.body, color = palette.accent) }
                    }
                }
                else -> {
                    Column(Modifier.fillMaxWidth().card(12.dp, palette.card)) {
                        GroupRow(true) {
                            Text(
                                buildAnnotatedString {
                                    append("On the hub's machine run ")
                                    withStyle(SpanStyle(fontFamily = FontFamily.Monospace)) { append("switchboard admin pair-device") }
                                    append(", then scan its QR code or paste the link.")
                                },
                                style = Type.callout, color = palette.label,
                            )
                        }
                        Column(Modifier.fillMaxWidth().clickable {
                            cameraNote = null
                            val options = GmsBarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()
                            GmsBarcodeScanning.getClient(context, options).startScan()
                                .addOnSuccessListener { code ->
                                    val raw = code.rawValue.orEmpty()
                                    if (PairingLink.parse(raw) != null) linkText = raw
                                    else cameraNote = "That QR code isn't a Switchboard pairing link."
                                }
                                .addOnFailureListener { e ->
                                    cameraNote = if ((e as? MlKitException)?.errorCode == MlKitException.CODE_SCANNER_CANCELLED) null
                                    else "The scanner isn't available on this phone. Paste the link instead."
                                }
                        }) {
                            GroupRow(false) {
                                Text("Scan QR code", style = Type.body, color = palette.accent)
                                cameraNote?.let { Text(it, style = Type.caption, color = palette.needs) }
                            }
                        }
                        GroupRow(false) {
                            Field(linkText, { linkText = it }, "switchboard://pair?…", "pairLink", mono = true)
                        }
                        GroupRow(false) {
                            Field(name, { name = it }, "This phone's name", "deviceName", mono = false)
                        }
                    }
                    model.pairingError?.let {
                        Text(it, Modifier.fillMaxWidth().card(12.dp, palette.card).padding(16.dp), style = Type.body, color = palette.red)
                    }
                    val link = PairingLink.parse(linkText)
                    val enabled = link != null && name.isNotBlank()
                    Column(
                        Modifier.fillMaxWidth().card(12.dp, palette.card)
                            .clickable(enabled = enabled) { link?.let { model.pair(it, name.trim()) } }.testTag("pairButton"),
                    ) {
                        GroupRow(true) { Text("Pair", style = Type.body, color = if (enabled) palette.accent else palette.tertiary) }
                    }
                }
            }
        }
    }
}

/** A plain text field inside a grouped row, as iOS forms have. */
@Composable
private fun Field(value: String, onChange: (String) -> Unit, placeholder: String, tag: String, mono: Boolean) {
    val style = TextStyle(
        fontSize = if (mono) Type.footnote.fontSize else Type.body.fontSize,
        fontFamily = if (mono) FontFamily.Monospace else FontFamily.Default,
        color = palette.label,
    )
    BasicTextField(
        value, onChange,
        Modifier.fillMaxWidth().testTag(tag),
        textStyle = style,
        cursorBrush = SolidColor(palette.accent),
        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false),
        decorationBox = { inner ->
            if (value.isEmpty()) Text(placeholder, style = style.copy(color = palette.tertiary))
            inner()
        },
    )
}
