package com.digitaltracer.switchboard.core

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class AnsiTest {
    @Test
    fun aPromptKeepsItsShadedBand() {
        val runs = Ansi.parse(
            "\u001b[0m\u001b[38;2;153;153;153m\u001b[48;2;55;55;55m❯ \u001b[0m\u001b[38;2;255;255;255m\u001b[48;2;55;55;55mpush both\u001b[0m\n  done",
        )
        assertEquals(listOf("❯ ", "push both", "\n  done"), runs.map { it.text })
        assertEquals(Ansi.Color(55, 55, 55), runs[1].style.bg)
        assertEquals(Ansi.Color(255, 255, 255), runs[1].style.fg)
        assertEquals(Ansi.Style(), runs[2].style)
    }

    @Test
    fun paletteBoldAndOtherCodes() {
        val runs = Ansi.parse("\u001b[1;38;5;196mred\u001b[22;39m plain \u001b[92mgreen\u001b[2Jgone")
        assertEquals(listOf("red", " plain ", "greengone"), runs.map { it.text })
        assertTrue(runs[0].style.bold)
        assertEquals(Ansi.Color(255, 0, 0), runs[0].style.fg)
        assertEquals(null, runs[1].style.fg)
        assertEquals(Ansi.palette(10), runs[2].style.fg)
    }
}
