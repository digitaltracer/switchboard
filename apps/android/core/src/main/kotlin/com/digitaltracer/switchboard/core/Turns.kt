package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.Skill
import com.digitaltracer.switchboard.api.models.TranscriptItem
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

/**
 * A conversation as a phone shows it: your messages and the agent's replies, with the steps in
 * between folded into one piece of work that says what came of it.
 */
object Turns {
    sealed interface Entry {
        val id: String

        data class User(val item: TranscriptItem) : Entry { override val id get() = item.id }
        data class Reply(val item: TranscriptItem) : Entry { override val id get() = item.id }
        /** Something the agent said with more work after it before your next message: a note on the way. */
        data class Progress(val item: TranscriptItem) : Entry { override val id get() = item.id }
        data class Note(val item: TranscriptItem) : Entry { override val id get() = item.id }
        data class Work(val items: List<TranscriptItem>, val seconds: Long?, val tags: List<Tag>) : Entry {
            override val id get() = "work:${items.firstOrNull()?.id.orEmpty()}"
            val running get() = items.any { !it.done }
        }
    }

    enum class Tone { Plain, Good, Bad }

    data class Tag(val text: String, val added: Int? = null, val removed: Int? = null, val tone: Tone = Tone.Plain)

    data class Latest(val reply: TranscriptItem?, val tags: List<Tag>, val at: String?)

    fun entries(items: List<TranscriptItem>): List<Entry> {
        val out = mutableListOf<Entry>()
        val steps = mutableListOf<TranscriptItem>()
        fun flush() {
            if (steps.isNotEmpty()) out += work(steps.toList())
            steps.clear()
        }
        for (item in items) {
            when (item.kind) {
                "tool" -> steps += item
                "user" -> { flush(); out += Entry.User(item) }
                "assistant" -> { flush(); out += Entry.Reply(item) }
                else -> { flush(); out += Entry.Note(item) }
            }
        }
        flush()
        // A reply with work or another reply after it, before your next message or a command, was on
        // the way, unless the log says it ended the turn: then something unseen started that work.
        var followed = false
        for (i in out.indices.reversed()) {
            when (val e = out[i]) {
                is Entry.User, is Entry.Note -> followed = false
                is Entry.Work -> followed = true
                is Entry.Reply -> if (followed && e.item.endsTurn != true) out[i] = Entry.Progress(e.item) else followed = true
                else -> {}
            }
        }
        return out
    }

    /** Since your last message: the agent's latest reply and what its work came to. */
    fun latest(items: List<TranscriptItem>): Latest {
        val since = items.indexOfLast { it.kind == "user" } + 1
        val recent = items.drop(since)
        val reply = recent.lastOrNull { it.kind == "assistant" } ?: items.lastOrNull { it.kind == "assistant" }
        val steps = recent.filter { it.kind == "tool" }
        return Latest(reply, if (steps.isEmpty()) emptyList() else tags(steps), reply?.at)
    }

    private fun work(steps: List<TranscriptItem>): Entry.Work {
        val times = steps.mapNotNull { it.at?.let(::instant) }
        val seconds = if (times.isEmpty()) null else times.max().epochSecond - times.min().epochSecond
        return Entry.Work(steps, seconds, tags(steps))
    }

    fun tags(steps: List<TranscriptItem>): List<Tag> {
        val out = mutableListOf<Tag>()
        val edits = steps.filter { it.tool in EDITS }
        if (edits.isNotEmpty()) {
            val files = edits.map { it.text ?: it.id }.toSet().size
            val counted = edits.any { it.added != null || it.removed != null }
            out += Tag(
                if (files == 1) "1 file" else "$files files",
                added = if (counted) edits.sumOf { it.added ?: 0 } else null,
                removed = if (counted) edits.sumOf { it.removed ?: 0 } else null,
            )
        }
        val tests = steps.filter { isRun(it) && TEST.containsMatchIn(it.text.orEmpty()) }
        if (tests.isNotEmpty() && tests.all { it.done }) {
            out += if (tests.any { it.failed }) Tag("Tests failed", tone = Tone.Bad) else Tag("Tests passed", tone = Tone.Good)
        }
        steps.mapNotNull(::pullRequest).lastOrNull()?.let { out += Tag("PR #$it") }
        if (steps.any { isRun(it) && it.text.orEmpty().contains("git push") && it.done && !it.failed }) out += Tag("Pushed")
        val failed = steps.count { it.failed && it !in tests }
        if (failed > 0) out += Tag("$failed failed", tone = Tone.Bad)
        if (out.isEmpty()) {
            val runs = steps.count(::isRun)
            out += Tag(
                when {
                    runs == 1 -> "1 command"
                    runs > 1 -> "$runs commands"
                    steps.size == 1 -> "1 step"
                    else -> "${steps.size} steps"
                },
            )
        }
        return out
    }

    private val EDITS = setOf("Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch")
    private val TEST = Regex("""\b(pytest|cargo test|cargo nextest|go test|swift test|npm (run )?test|yarn test|pnpm test|jest|vitest|rspec|mvn test|gradlew\b.*\btest)""")
    private val PULL = Regex("""github\.com/[^/\s]+/[^/\s]+/pull/(\d+)""")

    private fun isRun(i: TranscriptItem) = i.text.orEmpty().startsWith("Ran ")

    private fun pullRequest(i: TranscriptItem): String? =
        listOfNotNull(i.output, i.text).firstNotNullOfOrNull { PULL.find(it)?.groupValues?.get(1) }

    private fun instant(s: String): Instant? = runCatching { Instant.parse(s) }.getOrNull()

    /** When something was written, as the phone's clock shows times: "7:55 PM". */
    fun time(at: String): String? = instant(at)?.atZone(ZoneId.systemDefault())
        ?.format(DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT))

    /** "6m", "9s" or "1h 5m". */
    fun duration(seconds: Long): String = when {
        seconds < 60 -> "${maxOf(seconds, 1)}s"
        seconds < 3600 -> "${seconds / 60}m"
        else -> "${seconds / 3600}h ${seconds % 3600 / 60}m"
    }
}

/** The skills an agent can run, filtered as you type after `/`. */
object SkillSearch {
    private fun bare(s: String) = s.trimStart('/', '$').lowercase()

    /** Names starting with what's typed first, then names containing it, then descriptions. */
    fun filter(skills: List<Skill>, typed: String): List<Skill> {
        val q = bare(typed)
        if (q.isEmpty()) return skills
        val starts = skills.filter { bare(it.name).startsWith(q) }
        val inName = skills.filter { !bare(it.name).startsWith(q) && bare(it.name).contains(q) }
        val inDescription = skills.filter { !bare(it.name).contains(q) && it.description.lowercase().contains(q) }
        return starts + inName + inDescription
    }

    /** Whether a draft is a skill being typed: a `/` or `$` and no space yet. */
    fun isTyping(text: String): Boolean =
        (text.startsWith('/') || text.startsWith('$')) && !text.contains(' ') && !text.contains('\n')
}
