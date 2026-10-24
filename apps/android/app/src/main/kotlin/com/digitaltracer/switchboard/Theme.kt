package com.digitaltracer.switchboard

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.ExperimentalTextApi
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

/** iOS's system colours, so the app looks like the iPhone app (design E). */
@Immutable
data class Palette(
    val background: Color,
    val card: Color,
    val fill: Color,
    val separator: Color,
    val label: Color,
    val secondary: Color,
    val tertiary: Color,
    val accent: Color,
    val red: Color,
    val needs: Color,
    val working: Color,
    val done: Color,
    val bar: Color,
)

private val light = Palette(
    background = Color(0xFFF2F2F7),
    card = Color.White,
    fill = Color(0x1F767680),
    separator = Color(0x4A3C3C43),
    label = Color.Black,
    secondary = Color(0x993C3C43),
    tertiary = Color(0x4D3C3C43),
    accent = Color(0xFF007AFF),
    red = Color(0xFFFF3B30),
    // Text tints that keep 4.5:1 on light backgrounds.
    needs = Color(0xFFB34F00),
    working = Color(0xFF0059B0),
    done = Color(0xFF1F7D33),
    bar = Color(0xF2F9F9F9),
)

private val dark = Palette(
    background = Color.Black,
    card = Color(0xFF1C1C1E),
    fill = Color(0x3D767680),
    separator = Color(0xA6545458),
    label = Color.White,
    secondary = Color(0x99EBEBF5),
    tertiary = Color(0x4DEBEBF5),
    accent = Color(0xFF0A84FF),
    red = Color(0xFFFF453A),
    needs = Color(0xFFFF9F0A),
    working = Color(0xFF0A84FF),
    done = Color(0xFF30D158),
    bar = Color(0xF21D1D1D),
)

val LocalPalette = staticCompositionLocalOf { light }

/** Inter, the open-source face closest to Apple's SF, from one variable font file. */
@OptIn(ExperimentalTextApi::class)
val Inter = FontFamily(
    listOf(FontWeight.Normal, FontWeight.Medium, FontWeight.SemiBold, FontWeight.Bold).map { w ->
        Font(R.font.inter, w, variationSettings = FontVariation.Settings(FontVariation.weight(w.weight)))
    },
)

/** iOS's text styles at the default size. */
object Type {
    private fun style(size: Int, line: Int, weight: FontWeight = FontWeight.Normal) =
        TextStyle(fontFamily = Inter, fontSize = size.sp, lineHeight = line.sp, fontWeight = weight)

    val largeTitle = style(34, 41, FontWeight.Bold)
    val title3 = style(20, 25, FontWeight.Bold)
    val headline = style(17, 22, FontWeight.SemiBold)
    val body = style(17, 22)
    val callout = style(16, 21)
    val subheadline = style(15, 20)
    val footnote = style(13, 18)
    val caption = style(12, 16)
}

private fun Typography.withFamily(f: FontFamily) = Typography(
    displayLarge.copy(fontFamily = f), displayMedium.copy(fontFamily = f), displaySmall.copy(fontFamily = f),
    headlineLarge.copy(fontFamily = f), headlineMedium.copy(fontFamily = f), headlineSmall.copy(fontFamily = f),
    titleLarge.copy(fontFamily = f), titleMedium.copy(fontFamily = f), titleSmall.copy(fontFamily = f),
    bodyLarge.copy(fontFamily = f), bodyMedium.copy(fontFamily = f), bodySmall.copy(fontFamily = f),
    labelLarge.copy(fontFamily = f), labelMedium.copy(fontFamily = f), labelSmall.copy(fontFamily = f),
)

@Composable
fun SwitchboardTheme(isDark: Boolean, content: @Composable () -> Unit) {
    val p = if (isDark) dark else light
    val scheme = (if (isDark) darkColorScheme() else lightColorScheme()).copy(
        primary = p.accent,
        onPrimary = Color.White,
        background = p.background,
        surface = p.background,
        surfaceContainer = p.card,
        surfaceContainerHigh = p.card,
        surfaceContainerHighest = p.card,
        onSurface = p.label,
        onSurfaceVariant = p.secondary,
        outlineVariant = p.separator,
        error = p.red,
    )
    CompositionLocalProvider(LocalPalette provides p) {
        MaterialTheme(colorScheme = scheme, typography = Typography().withFamily(Inter), content = content)
    }
}
