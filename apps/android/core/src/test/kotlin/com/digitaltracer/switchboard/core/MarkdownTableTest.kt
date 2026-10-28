package com.digitaltracer.switchboard.core

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class MarkdownTableTest {
    @Test
    fun aTableKeepsItsHeaderAndSizesColumnsByTheirLongestCell() {
        val t = MarkdownTable.parse(
            """
            | Fix | Commit |
            |---|:---:|
            | The FastAPI recipe now warns that it stops the middleware's exceptions | `41062ee` |
            | Pipes \| stay | `a1b2c3d` |
            | Short |
            """.trimIndent(),
        )!!
        assertEquals(listOf("Fix", "Commit"), t.header)
        assertEquals(3, t.rows.size)
        assertEquals("Pipes | stay", t.rows[1][0])
        assertEquals(listOf("Short", ""), t.rows[2])
        assertEquals(listOf(40f, 9f), t.weights)
    }

    @Test
    fun aTableWithoutARuleHasNoHeader() {
        val t = MarkdownTable.parse("| a | b |\n| c | d |")!!
        assertTrue(t.header.isEmpty() && t.rows.size == 2)
        assertNull(MarkdownTable.parse("no table here"))
    }
}
