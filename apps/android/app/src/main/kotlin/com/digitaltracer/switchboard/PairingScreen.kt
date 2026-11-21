package com.digitaltracer.switchboard

import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.QrCodeScanner
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
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

    val confirming = model.phase as? AppModel.Phase.Confirming
    InboxPage(
        top = {
            Row(
                Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.statusBars).height(44.dp).padding(horizontal = 20.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("Switchboard", Modifier.weight(1f), style = Type.meta.copy(fontWeight = FontWeight.Medium), color = palette.sub)
                Text(if (confirming == null) "Step 1 of 2" else "Step 2 of 2", style = Type.meta, color = palette.mut)
            }
        },
        bottom = {
            Column(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                if (confirming != null) {
                    BigButton("Cancel", Modifier.fillMaxWidth().testTag("cancelPairing"), ButtonKind.Outline, onClick = model::cancelPairing)
                } else {
                    val link = PairingLink.parse(linkText)
                    BigButton("Pair", Modifier.fillMaxWidth().testTag("pairButton"), enabled = link != null && name.isNotBlank()) {
                        link?.let { model.pair(it, name.trim()) }
                    }
                    Note("Everything stays on your machine. The relay only forwards encrypted requests.", Modifier.fillMaxWidth())
                }
            }
        },
    ) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (confirming != null) {
                StateLine(stateText("Confirm on your computer."), Modifier.padding(top = 28.dp))
                Text(
                    buildAnnotatedString {
                        append("Check that the terminal running pair-device shows exactly this fingerprint, then type ")
                        withStyle(SpanStyle(fontFamily = GeistMono, color = palette.ink)) { append("yes") }
                        append(" there.")
                    },
                    style = Type.say, color = palette.sub,
                )
                CardBox(Modifier.padding(top = 18.dp), padding = 22.dp) {
                    Text("THIS PHONE’S FINGERPRINT", style = Type.section, color = palette.mut)
                    val groups = confirming.fingerprint.split("-")
                    Text(
                        if (groups.size > 2) groups.chunked(2).joinToString("\n") { it.joinToString("-") } else confirming.fingerprint,
                        Modifier.testTag("fingerprint"), style = Type.mono(26f, FontWeight.Medium).copy(lineHeight = 35.sp), color = palette.ink,
                    )
                    Hairline()
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        StatusGlyph(Glyph.Work)
                        Text("Waiting for the hub…", style = Type.text, color = palette.sub)
                    }
                }
                Note("If the two don’t match, cancel. Someone else may have used your pairing link.", Modifier.padding(top = 4.dp))
            } else {
                StateLine(stateText("Pair with your hub."), Modifier.padding(top = 28.dp).testTag("pairTitle"))
                Text(
                    buildAnnotatedString {
                        append("On the hub’s machine run ")
                        withStyle(SpanStyle(fontFamily = GeistMono, color = palette.ink)) { append("switchboard admin pair-device") }
                        append(", then scan the QR code it prints.")
                    },
                    Modifier.padding(bottom = 18.dp), style = Type.say, color = palette.sub,
                )
                BigButton("Scan QR code", Modifier.fillMaxWidth(), icon = Icons.Outlined.QrCodeScanner) {
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
                }
                cameraNote?.let { Note(it, color = palette.warn) }
                SectionHead("Or paste the link", top = 16.dp)
                Note("Pairing link")
                Field(linkText, { linkText = it }, "switchboard://pair?…", "pairLink", mono = true)
                Note("Name this phone", Modifier.padding(top = 8.dp))
                Field(name, { name = it }, "This phone's name", "deviceName", mono = false)
                Note("The name shows in the hub’s device list, so you can revoke this phone later.")
                model.pairingError?.let { Banner(it, Modifier.padding(top = 6.dp).testTag("pairingError")) }
            }
        }
    }
}

/** A bordered text field. */
@Composable
private fun Field(value: String, onChange: (String) -> Unit, placeholder: String, tag: String, mono: Boolean) {
    val style = (if (mono) Type.mono(14f) else Type.field).copy(color = palette.ink)
    Box(
        Modifier.fillMaxWidth().heightIn(min = 52.dp).clip(RoundedCornerShape(14.dp)).background(palette.sf)
            .border(1.dp, palette.line, RoundedCornerShape(14.dp)).padding(horizontal = 16.dp, vertical = 14.dp),
        Alignment.CenterStart,
    ) {
        BasicTextField(
            value, onChange,
            Modifier.fillMaxWidth().testTag(tag),
            textStyle = style,
            cursorBrush = SolidColor(palette.ac),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false),
            decorationBox = { inner ->
                if (value.isEmpty()) Text(placeholder, style = style.copy(color = palette.mut))
                inner()
            },
        )
    }
}
