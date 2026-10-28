package com.digitaltracer.switchboard.core

import kotlin.test.Test
import kotlin.test.assertEquals

class MarkdownTest {
    @Test
    fun listsKeepTheirNestingAndWrappedLinesJoinTheirItem() {
        val text = """
            ## Round 2

            ### Findings

            I found no high defects.
            - **r2-1** · `client.py:181` · low
              - State: a retried call
              with an expired token.
              - Fix (a): say so.
            - **r2-2** · test gap

            1. First
            2. Second
            ---
            After.
        """.trimIndent()
        val blocks = Markdown.blocks(text)
        assertEquals(
            listOf(
                Markdown.Block.Heading(2, "Round 2"),
                Markdown.Block.Heading(3, "Findings"),
                Markdown.Block.Paragraph("I found no high defects."),
                Markdown.Block.List(
                    listOf(
                        Markdown.Item(0, "•", "**r2-1** · `client.py:181` · low"),
                        Markdown.Item(1, "•", "State: a retried call with an expired token."),
                        Markdown.Item(1, "•", "Fix (a): say so."),
                        Markdown.Item(0, "•", "**r2-2** · test gap"),
                        Markdown.Item(0, "1.", "First"),
                        Markdown.Item(0, "2.", "Second"),
                    ),
                ),
                Markdown.Block.Rule,
                Markdown.Block.Paragraph("After."),
            ),
            blocks,
        )
    }

    @Test
    fun codeAndTablesStayWhole() {
        val text = "Run:\n```\n- not a list\n```\n| a | b |\n|---|---|\n| 1 | 2 |\nAfter"
        assertEquals(
            listOf(
                Markdown.Block.Paragraph("Run:"),
                Markdown.Block.Code("- not a list"),
                Markdown.Block.Table("| a | b |\n|---|---|\n| 1 | 2 |"),
                Markdown.Block.Paragraph("After"),
            ),
            Markdown.blocks(text),
        )
    }

    @Test
    fun aParagraphAfterABlankLineEndsTheList() {
        assertEquals(
            listOf(Markdown.Block.List(listOf(Markdown.Item(0, "•", "one"))), Markdown.Block.Paragraph("Then this.")),
            Markdown.blocks("- one\n\nThen this."),
        )
    }
}
