package com.digitaltracer.switchboard.core

/** A Markdown table's cells, with each column's share of the width from its longest cell. */
data class MarkdownTable(val header: List<String>, val rows: List<List<String>>, val weights: List<Float>) {
    val columns get() = weights.size

    companion object {
        /** `| a | b |` lines, the `|---|` line under the header marking it. Null when no line has cells. */
        fun parse(text: String): MarkdownTable? {
            val lines = text.split("\n").map(::cells).filter { it.isNotEmpty() }.toMutableList()
            if (lines.isEmpty()) return null
            var header = emptyList<String>()
            if (lines.size > 1 && lines[1].all(::isRule)) {
                header = lines[0]
                lines.removeAt(0)
                lines.removeAt(0)
            }
            val columns = (listOf(header) + lines).maxOf { it.size }
            val pad = { r: List<String> -> r + List(columns - r.size) { "" } }
            val rows = lines.map(pad)
            val all = (if (header.isEmpty()) emptyList() else listOf(pad(header))) + rows
            val weights = (0 until columns).map { c -> all.maxOf { it[c].length }.coerceIn(3, 40).toFloat() }
            return MarkdownTable(if (header.isEmpty()) emptyList() else pad(header), rows, weights)
        }

        /** A row's cells; `\|` is a literal bar. */
        internal fun cells(line: String): List<String> {
            var s = line.trim()
            if (!s.startsWith("|")) return emptyList()
            s = s.drop(1)
            if (s.endsWith("|") && !s.endsWith("\\|")) s = s.dropLast(1)
            val marker = "\u0000"
            return s.replace("\\|", marker).split("|").map { it.replace(marker, "|").trim() }
        }

        private fun isRule(cell: String) = cell.isNotEmpty() && cell.all { it == '-' || it == ':' } && '-' in cell
    }
}
