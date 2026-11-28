package com.digitaltracer.switchboard

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Edit
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import com.digitaltracer.switchboard.core.AgentSpace
import com.digitaltracer.switchboard.core.HerdrSession
import com.digitaltracer.switchboard.core.Names
import com.digitaltracer.switchboard.core.Offer
import com.digitaltracer.switchboard.core.isHerdrAgent
import com.digitaltracer.switchboard.core.nameInSpace
import com.digitaltracer.switchboard.core.rename
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/**
 * One text field for a new name. Save stays off until the name fits; a failure stays in the
 * sheet, in words, with what was typed. [herdrAgent] is an agent in a Herdr terminal, whose
 * names Herdr restricts. [save] sends the trimmed name and throws to keep the sheet open.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RenameSheet(title: String, current: String, herdrAgent: Boolean = false, dismiss: () -> Unit, save: suspend (String) -> Unit) {
    var name by remember { mutableStateOf(TextFieldValue(current, TextRange(current.length))) }
    var error by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val focus = remember { FocusRequester() }
    val trimmed = name.text.trim()
    val problem = Names.problem(name.text, herdrAgent)
    val canSave = problem == null && trimmed != current && !saving

    fun send() {
        if (!canSave) return
        scope.launch {
            saving = true
            try {
                save(trimmed)
                dismiss()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = Names.failure(e)
            } finally {
                saving = false
            }
        }
    }

    ModalBottomSheet(
        dismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = palette.sf, shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
    ) {
        Column(Modifier.imePadding().padding(horizontal = 20.dp).padding(bottom = 16.dp).windowInsetsPadding(WindowInsets.navigationBars)) {
            Text(title, style = Type.h1, color = palette.ink)
            Box(
                Modifier.padding(top = 16.dp).fillMaxWidth().heightIn(min = 52.dp).clip(RoundedCornerShape(14.dp)).background(palette.bg)
                    .border(1.dp, palette.line, RoundedCornerShape(14.dp)).padding(horizontal = 16.dp),
                Alignment.CenterStart,
            ) {
                BasicTextField(
                    name, { name = it },
                    Modifier.fillMaxWidth().focusRequester(focus).testTag("renameField"),
                    textStyle = Type.field.copy(color = palette.ink),
                    cursorBrush = SolidColor(palette.ac),
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(
                        capitalization = if (herdrAgent) KeyboardCapitalization.None else KeyboardCapitalization.Sentences,
                        autoCorrectEnabled = false, imeAction = ImeAction.Done,
                    ),
                    keyboardActions = KeyboardActions(onDone = { send() }),
                    decorationBox = { inner ->
                        if (name.text.isEmpty()) Text("Name", style = Type.field, color = palette.mut)
                        inner()
                    },
                )
            }
            val shown = error
            when {
                shown != null -> Note(shown, Modifier.padding(top = 10.dp).testTag("renameError"), palette.bad)
                herdrAgent -> Note(Names.HERDR_AGENT_RULE, Modifier.padding(top = 10.dp))
                trimmed.isNotEmpty() && problem != null -> Note(problem, Modifier.padding(top = 10.dp), palette.warn)
            }
            Column(Modifier.padding(top = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                BigButton("Save", Modifier.fillMaxWidth().testTag("renameSave"), enabled = canSave, busy = saving) { send() }
                BigButton("Cancel", Modifier.fillMaxWidth(), ButtonKind.Plain, color = palette.sub, onClick = dismiss)
            }
        }
    }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
}

/**
 * A "Name" row under "At your desk": the name with Rename beside it, or why it can't be renamed
 * from the phone. Nothing when there is nothing to offer or say.
 */
@Composable
fun NameRow(name: String, offer: Offer, rename: () -> Unit) {
    when (offer) {
        Offer.Yes -> FieldRow("Name", "") {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(name, Modifier.weight(1f, fill = false), style = Type.field, color = palette.sub, maxLines = 1)
                Text(
                    "Rename", Modifier.heightIn(min = 44.dp).clickable(onClick = rename).padding(vertical = 12.dp).testTag("rename"),
                    style = Type.sub.copy(fontWeight = FontWeight.Medium), color = palette.ac,
                )
            }
        }
        is Offer.No -> {
            FieldRow("Name", name)
            Note(offer.why, Modifier.padding(bottom = 11.dp).testTag("renameBlocked"), palette.sub)
        }
        Offer.Hidden -> {}
    }
}

/** Long-press on an agent's row: Rename, or why it can't be renamed from the phone. */
@Composable
fun RenameMenu(model: AppModel, agent: HerdrSession.Agent, row: @Composable (longPress: (() -> Unit)?) -> Unit) {
    val name = (agent as? HerdrSession.Agent.Task)?.session?.label ?: agent.nameInSpace
    RenameMenu(name, agent.rename, agent.isHerdrAgent, { new, id -> model.rename(agent, new, id) }, row)
}

/** Long-press on a space's row: Rename, or why it can't be renamed from the phone. */
@Composable
fun RenameMenu(model: AppModel, space: AgentSpace, row: @Composable (longPress: (() -> Unit)?) -> Unit) =
    RenameMenu(space.key.space ?: space.title, space.rename, false, { new, id -> model.rename(space, new, id) }, row)

@Composable
private fun RenameMenu(
    name: String, offer: Offer, herdrAgent: Boolean, save: suspend (String, String) -> Unit,
    row: @Composable (longPress: (() -> Unit)?) -> Unit,
) {
    if (offer == Offer.Hidden) return row(null)
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    // One request id per name tried, so a retry after a lost answer is the same request.
    val ids = remember { mutableMapOf<String, String>() }
    Box {
        row { menu = true }
        DropdownMenu(menu, { menu = false }) {
            if (offer is Offer.No) {
                DropdownMenuItem(
                    {
                        Column {
                            Text("Rename", style = Type.field, color = palette.mut)
                            Text(offer.why, style = Type.meta, color = palette.mut)
                        }
                    },
                    {}, enabled = false,
                )
            } else {
                DropdownMenuItem({ Text("Rename", style = Type.field) }, {
                    menu = false
                    renaming = true
                }, Modifier.testTag("renameMenu"), leadingIcon = { Icon(Icons.Outlined.Edit, null, tint = palette.ink) })
            }
        }
    }
    if (renaming) {
        RenameSheet("Rename $name", name, herdrAgent, dismiss = { renaming = false }) { new ->
            save(new, ids.getOrPut(new) { java.util.UUID.randomUUID().toString() })
        }
    }
}
