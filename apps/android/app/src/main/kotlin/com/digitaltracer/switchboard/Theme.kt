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
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp

/**
 * The Inbox design's colours, light and dark: a near-white ground, black primary buttons, one
 * blue for working and links, orange for needs you, green for done.
 */
@Immutable
data class Palette(
    val bg: Color,
    val sf: Color,
    val ink: Color,
    val sub: Color,
    val mut: Color,
    val line: Color,
    val fill: Color,
    val ac: Color,
    val warn: Color,
    val wbg: Color,
    val ok: Color,
    val bad: Color,
    val inv: Color,
    /** Behind code, in both appearances. */
    val code: Color,
) {
    // The names the conversation views grew up with.
    val background get() = bg
    val card get() = sf
    val separator get() = line
    val label get() = ink
    val secondary get() = sub
    val tertiary get() = mut
    val accent get() = ac
    val red get() = bad
    val needs get() = warn
    val working get() = ac
    val done get() = ok
    val bar get() = bg
    val body get() = ink
    val bodyDim get() = sub
    val codeFill get() = fill
}

private val light = Palette(
    bg = Color(0xFFF5F6F6), sf = Color.White, ink = Color(0xFF101214), sub = Color(0xFF4F555B), mut = Color(0xFF666C73),
    line = Color(0xFFE1E3E5), fill = Color(0xFFECEEEF), ac = Color(0xFF2743D6), warn = Color(0xFFB4460A),
    wbg = Color(0xFFFFF1E6), ok = Color(0xFF1F7A4D), bad = Color(0xFFC0362C), inv = Color.White, code = Color(0xFF101214),
)

private val dark = Palette(
    bg = Color(0xFF0C0D0F), sf = Color(0xFF16181B), ink = Color(0xFFF2F3F4), sub = Color(0xFFA9AEB4), mut = Color(0xFF8C9299),
    line = Color(0xFF25282C), fill = Color(0xFF1E2125), ac = Color(0xFF93A5FF), warn = Color(0xFFFFA468),
    wbg = Color(0xFF2B1C11), ok = Color(0xFF6FD3A0), bad = Color(0xFFFF8F85), inv = Color(0xFF0C0D0F), code = Color.Black,
)

/** The terminal's own ground and text, the same in both appearances. */
val termBg = Color(0xFF0C0D0F)
val codeText = Color(0xFFF2F3F4)
val codeDim = Color(0xFF8C9299)

val LocalPalette = staticCompositionLocalOf { light }

/** Geist for text, Geist Mono for commands, paths and fingerprints; both bundled. */
val Geist = FontFamily(
    Font(R.font.geist_regular, FontWeight.Normal),
    Font(R.font.geist_medium, FontWeight.Medium),
    Font(R.font.geist_semibold, FontWeight.SemiBold),
    Font(R.font.geist_bold, FontWeight.Bold),
)

val GeistMono = FontFamily(
    Font(R.font.geist_mono_regular, FontWeight.Normal),
    Font(R.font.geist_mono_medium, FontWeight.Medium),
    Font(R.font.geist_mono_semibold, FontWeight.SemiBold),
)

/** The design's type, from its stylesheet. */
object Type {
    private fun style(size: Float, line: Float, weight: FontWeight = FontWeight.Normal, tracking: Float = -0.005f) =
        TextStyle(fontFamily = Geist, fontSize = size.sp, lineHeight = line.sp, fontWeight = weight, letterSpacing = tracking.em)

    fun mono(size: Float, weight: FontWeight = FontWeight.Normal) =
        TextStyle(fontFamily = GeistMono, fontSize = size.sp, lineHeight = (size * 1.4f).sp, fontWeight = weight)

    /** The sentence at the top of a page, "3 things need you." */
    val state = style(34f, 37f, FontWeight.SemiBold, -0.035f)
    val h1 = style(28f, 32f, FontWeight.SemiBold, -0.03f)
    val title = style(16f, 21f, FontWeight.Medium, -0.01f)
    val text = style(15f, 21f)
    val sub = style(13.5f, 18f)
    val meta = style(13f, 18f)
    val section = style(12f, 16f, FontWeight.Medium, 0.07f)
    val button = style(16f, 20f, FontWeight.SemiBold)
    val say = style(16f, 24f)
    val draft = style(22f, 28.6f, FontWeight.Medium, -0.02f)
    val field = style(16f, 22f)

    // The names the conversation views grew up with.
    val largeTitle = state
    val pageTitle = h1
    val title3 = style(20f, 26f, FontWeight.Medium, -0.02f)
    val headline = title
    val body = field
    val callout = field
    val subheadline = sub
    val footnote = meta
    val caption = style(12f, 16f)

    // Long agent text, with room between lines.
    val reply = style(16f, 24f)
    val replyHeading = style(16f, 22f, FontWeight.SemiBold)
    /** A smaller heading in a reply, in capitals. */
    val replySection = style(13f, 18f, FontWeight.Medium, 0.06f)
    /** What the agent said on the way to its answer. */
    val progress = style(14.5f, 21f)
    val message = style(15.5f, 22f)
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
        primary = p.ink,
        onPrimary = p.inv,
        background = p.bg,
        surface = p.bg,
        surfaceContainer = p.sf,
        surfaceContainerLow = p.sf,
        surfaceContainerHigh = p.sf,
        surfaceContainerHighest = p.sf,
        onSurface = p.ink,
        onSurfaceVariant = p.sub,
        outlineVariant = p.line,
        error = p.bad,
    )
    CompositionLocalProvider(LocalPalette provides p) {
        MaterialTheme(colorScheme = scheme, typography = Typography().withFamily(Geist), content = content)
    }
}
