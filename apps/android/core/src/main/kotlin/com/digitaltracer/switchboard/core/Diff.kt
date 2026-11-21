package com.digitaltracer.switchboard.core

/** A diff hunk as the phone draws it: lines, with long runs of unchanged ones folded. */
sealed interface DiffRow {
    data class Line(val index: Int) : DiffRow
    /** Lines [from] until [until] of the hunk, unchanged and hidden until tapped. */
    data class Fold(val from: Int, val until: Int) : DiffRow {
        val count get() = until - from
    }
}

object DiffFolding {
    /** Unchanged lines kept beside each change. */
    const val KEEP = 3
    /** A run is folded only when at least this many lines would hide. */
    const val MIN_FOLD = 6

    /** [kinds] are the hunk's line kinds (`" "`, `"+"`, `"-"`); [open] lists folds already shown. */
    fun rows(kinds: List<String>, open: Set<Int> = emptySet()): List<DiffRow> {
        val out = mutableListOf<DiffRow>()
        var i = 0
        while (i < kinds.size) {
            if (kinds[i] != " ") {
                out += DiffRow.Line(i)
                i++
                continue
            }
            var j = i
            while (j < kinds.size && kinds[j] == " ") j++
            val hideFrom = i + if (i == 0) 0 else KEEP
            val hideTo = j - if (j == kinds.size) 0 else KEEP
            if (hideTo - hideFrom >= MIN_FOLD && hideFrom !in open) {
                (i until hideFrom).forEach { out += DiffRow.Line(it) }
                out += DiffRow.Fold(hideFrom, hideTo)
                (hideTo until j).forEach { out += DiffRow.Line(it) }
            } else {
                (i until j).forEach { out += DiffRow.Line(it) }
            }
            i = j
        }
        return out
    }
}

/** The message "Ask agent" sends: the file, the line numbers and the lines, then the question. */
object DiffQuote {
    fun message(path: String, lines: List<Pair<Int, String>>, question: String): String =
        "${location(path, lines.map { it.first })}:\n```\n${lines.joinToString("\n") { it.second }}\n```\n\n${question.trim()}"

    /** `src/a.ts:42–43`, or `src/a.ts:42` for one line. */
    fun location(path: String, numbers: List<Int>): String {
        val lo = numbers.minOrNull() ?: return path
        val hi = numbers.max()
        return if (lo == hi) "$path:$lo" else "$path:$lo–$hi"
    }
}

/** Bytes as the Changes list shows a binary file's size. */
fun fileSize(bytes: Long): String = when {
    bytes < 1000 -> "$bytes bytes"
    bytes < 1_000_000 -> "${(bytes + 500) / 1000} KB"
    else -> "%.1f MB".format(bytes / 1_000_000.0)
}
