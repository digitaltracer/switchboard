package com.digitaltracer.switchboard

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBackIos
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp

/**
 * An iOS-style screen: a bar with a back link and toolbar icons, then the content, which starts
 * with the large title. The bottom bar, if any, sits above the navigation bar.
 */
@Composable
fun Page(
    back: String?,
    onBack: () -> Unit = {},
    actions: @Composable RowScope.() -> Unit = {},
    bottomBar: (@Composable () -> Unit)? = null,
    content: @Composable () -> Unit,
) {
    Column(Modifier.fillMaxSize().background(palette.background)) {
        Row(
            Modifier.fillMaxWidth().windowInsetsPadding(WindowInsets.statusBars).height(44.dp).padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (back != null) {
                Row(
                    Modifier.clickable(onClick = onBack).padding(horizontal = 4.dp, vertical = 8.dp).testTag("back"),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(Icons.AutoMirrored.Filled.ArrowBackIos, null, Modifier.size(20.dp), tint = palette.accent)
                    Text(back, style = Type.body, color = palette.accent)
                }
            }
            Spacer(Modifier.weight(1f))
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically, content = actions)
        }
        Box(Modifier.weight(1f)) { content() }
        if (bottomBar != null) {
            Column(Modifier.fillMaxWidth().background(palette.bar).windowInsetsPadding(WindowInsets.navigationBars)) {
                HorizontalDivider(thickness = 0.5.dp, color = palette.separator)
                bottomBar()
            }
        }
    }
}

@Composable
fun LargeTitle(text: String, horizontal: Int = 16) {
    Text(text, Modifier.padding(start = horizontal.dp, end = horizontal.dp, bottom = 8.dp), style = Type.largeTitle, color = palette.label, maxLines = 2)
}

/** A toolbar icon in the accent colour. */
@Composable
fun BarIcon(icon: ImageVector, label: String, tag: String? = null, enabled: Boolean = true, tint: androidx.compose.ui.graphics.Color? = null, onClick: () -> Unit) {
    Box(
        Modifier.size(44.dp).clickable(enabled = enabled, onClick = onClick).semantics { contentDescription = label }
            .then(if (tag != null) Modifier.testTag(tag) else Modifier),
        Alignment.Center,
    ) {
        val color = tint ?: palette.accent
        Icon(icon, null, Modifier.size(24.dp), tint = if (enabled) color else palette.tertiary)
    }
}
