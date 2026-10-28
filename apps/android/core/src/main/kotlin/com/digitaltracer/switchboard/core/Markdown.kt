package com.digitaltracer.switchboard.core

/**
 * An agent's Markdown in the blocks a phone draws: paragraphs, headings, lists with their nesting,
 * code, tables and rules. Inline styles stay in each block's text.
 */
object Markdown {
    sealed interface Block {
        data class Paragraph(val text: String) : Block
        /** `#` is 1, `##` 2 and so on. */
        data class Heading(val level: Int, val text: String) : Block
        data class List(val items: kotlin.collections.List<Item>) : Block
        data class Code(val text: String) : Block
        data class Table(val text: String) : Block
        data object Rule : Block
    }

    /** [marker] is `•` for `-`, `*` and `+`, else the number as written, like `2.`. */
    data class Item(val level: Int, val marker: String, val text: String)

    private val ITEM = Regex("""^(\s*)([-*+]|\d{1,3}[.)])\s+(.*)$""")
    private val RULE = Regex("""^\s*([-*_])(\s*\1){2,}\s*$""")

    fun blocks(text: String): kotlin.collections.List<Block> {
        val out = mutableListOf<Block>()
        val para = mutableListOf<String>()
        val table = mutableListOf<String>()
        val items = mutableListOf<Item>()
        // The indents seen in this list, shallowest first; an item's level is where its indent ranks.
        val indents = mutableListOf<Int>()
        var code: MutableList<String>? = null
        var blank = false
        fun flush() {
            if (para.isNotEmpty()) out += Block.Paragraph(para.joinToString("\n")).also { para.clear() }
            if (table.isNotEmpty()) out += Block.Table(table.joinToString("\n")).also { table.clear() }
            if (items.isNotEmpty()) out += Block.List(items.toList()).also { items.clear() }
            indents.clear()
        }
        for (line in text.split("\n")) {
            val t = line.trim()
            val inCode = code
            val item = if (inCode == null) ITEM.find(line) else null
            val afterBlank = blank
            blank = inCode == null && t.isEmpty()
            when {
                t.startsWith("```") -> if (inCode != null) {
                    out += Block.Code(inCode.joinToString("\n"))
                    code = null
                } else {
                    flush(); code = mutableListOf()
                }
                inCode != null -> inCode += line
                t.startsWith("|") -> {
                    if (para.isNotEmpty() || items.isNotEmpty()) flush()
                    table += t
                }
                RULE.matches(line) -> { flush(); out += Block.Rule }
                t.startsWith("#") -> {
                    flush()
                    out += Block.Heading(t.takeWhile { it == '#' }.length.coerceAtMost(6), t.trimStart('#').trim())
                }
                t.isEmpty() -> if (items.isEmpty()) flush()
                item != null -> {
                    if (para.isNotEmpty() || table.isNotEmpty()) flush()
                    val indent = item.groupValues[1].length
                    while (indents.isNotEmpty() && indents.last() > indent) indents.removeAt(indents.lastIndex)
                    if (indents.isEmpty() || indents.last() < indent) indents += indent
                    val marker = item.groupValues[2].let { if (it in setOf("-", "*", "+")) "•" else it }
                    items += Item(indents.size - 1, marker, item.groupValues[3].trim())
                }
                // A line under an item continues it; after a blank line, only an indented one does.
                items.isNotEmpty() && (!afterBlank || line.startsWith(" ")) -> {
                    val last = items.removeAt(items.lastIndex)
                    items += last.copy(text = "${last.text} $t")
                }
                else -> {
                    if (items.isNotEmpty() || table.isNotEmpty()) flush()
                    para += line
                }
            }
        }
        code?.let { out += Block.Code(it.joinToString("\n")) }
        flush()
        return out
    }
}
