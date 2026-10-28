package com.digitaltracer.switchboard.core

/** A terminal screen's colours, from ANSI SGR codes. The hub sends only colour codes; anything else is dropped. */
object Ansi {
    data class Color(val r: Int, val g: Int, val b: Int)

    data class Style(
        val fg: Color? = null,
        val bg: Color? = null,
        val bold: Boolean = false,
        val dim: Boolean = false,
        val italic: Boolean = false,
        val underline: Boolean = false,
        val inverse: Boolean = false,
    )

    data class Run(val text: String, val style: Style)

    /** The text split into runs of one style each. */
    fun parse(s: String): List<Run> {
        val runs = mutableListOf<Run>()
        var style = Style()
        val text = StringBuilder()
        fun flush() {
            if (text.isNotEmpty()) runs += Run(text.toString(), style)
            text.clear()
        }
        var i = 0
        while (i < s.length) {
            val c = s[i]
            if (c != '\u001b') {
                text.append(c)
                i++
                continue
            }
            if (s.getOrNull(i + 1) != '[') {
                i += 2
                continue
            }
            var j = i + 2
            while (j < s.length && s[j].code !in 0x40..0x7e) j++
            if (j < s.length && s[j] == 'm') {
                flush()
                style = apply(s.substring(i + 2, j), style)
            }
            i = j + 1
        }
        flush()
        return runs
    }

    internal fun apply(params: String, start: Style): Style {
        var style = start
        val codes = ArrayDeque(params.split(';').map { it.toIntOrNull() ?: 0 })
        while (codes.isNotEmpty()) {
            when (val code = codes.removeFirst()) {
                0 -> style = Style()
                1 -> style = style.copy(bold = true)
                2 -> style = style.copy(dim = true)
                3 -> style = style.copy(italic = true)
                4 -> style = style.copy(underline = true)
                7 -> style = style.copy(inverse = true)
                22 -> style = style.copy(bold = false, dim = false)
                23 -> style = style.copy(italic = false)
                24 -> style = style.copy(underline = false)
                27 -> style = style.copy(inverse = false)
                in 30..37 -> style = style.copy(fg = palette(code - 30))
                in 90..97 -> style = style.copy(fg = palette(code - 90 + 8))
                39 -> style = style.copy(fg = null)
                in 40..47 -> style = style.copy(bg = palette(code - 40))
                in 100..107 -> style = style.copy(bg = palette(code - 100 + 8))
                49 -> style = style.copy(bg = null)
                38, 48 -> {
                    val color = extended(codes)
                    style = if (code == 38) style.copy(fg = color) else style.copy(bg = color)
                }
            }
        }
        return style
    }

    /** `5;n` from the 256-colour palette or `2;r;g;b`. */
    private fun extended(codes: ArrayDeque<Int>): Color? = when (codes.removeFirstOrNull()) {
        5 -> codes.removeFirstOrNull()?.let(::palette)
        2 -> if (codes.size < 3) null else Color(
            codes.removeFirst().coerceIn(0, 255), codes.removeFirst().coerceIn(0, 255), codes.removeFirst().coerceIn(0, 255),
        )
        else -> null
    }

    private val base = listOf(
        Color(0, 0, 0), Color(205, 49, 49), Color(13, 188, 121), Color(229, 229, 16),
        Color(36, 114, 200), Color(188, 63, 188), Color(17, 168, 205), Color(229, 229, 229),
        Color(102, 102, 102), Color(241, 76, 76), Color(35, 209, 139), Color(245, 245, 67),
        Color(59, 142, 234), Color(214, 112, 214), Color(41, 184, 219), Color(255, 255, 255),
    )

    /** xterm's 256 colours. */
    fun palette(n: Int): Color = when (n) {
        in 0..15 -> base[n]
        in 16..231 -> {
            val i = n - 16
            val level = { v: Int -> if (v == 0) 0 else 55 + v * 40 }
            Color(level(i / 36), level(i / 6 % 6), level(i % 6))
        }
        in 232..255 -> (8 + (n - 232) * 10).let { Color(it, it, it) }
        else -> Color(229, 229, 229)
    }
}
