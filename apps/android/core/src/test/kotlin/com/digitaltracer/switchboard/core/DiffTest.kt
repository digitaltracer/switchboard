package com.digitaltracer.switchboard.core

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertTrue

class DiffTest {
    @Test
    fun longUnchangedRunsFoldButKeepThreeLinesBesideEachChange() {
        val kinds = List(10) { " " } + "+" + List(20) { " " } + "-" + List(4) { " " }
        val rows = DiffFolding.rows(kinds)
        assertEquals(DiffRow.Fold(0, 7), rows.first())
        assertTrue(DiffRow.Fold(14, 28) in rows)
        val shown = DiffFolding.rows(kinds, open = setOf(14))
        assertTrue(DiffRow.Fold(14, 28) !in shown)
        assertEquals(listOf(DiffRow.Line(0), DiffRow.Line(1), DiffRow.Line(2)), DiffFolding.rows(listOf(" ", "+", " ")))
    }

    @Test
    fun askingQuotesTheFileTheLinesAndTheLinesThemselves() {
        val m = DiffQuote.message("src/importer/retry.ts", listOf(42 to "const delay = 2 ** attempt * 250;", 43 to "await sleep(delay);"), " Cap the delay at 30 seconds. ")
        assertEquals("src/importer/retry.ts:42–43:\n```\nconst delay = 2 ** attempt * 250;\nawait sleep(delay);\n```\n\nCap the delay at 30 seconds.", m)
        assertEquals("a.rs:7", DiffQuote.location("a.rs", listOf(7)))
        assertEquals("25 KB", fileSize(24576))
    }
}
