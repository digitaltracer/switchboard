package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.Transcript
import com.digitaltracer.switchboard.api.models.TranscriptItem

/**
 * An agent's conversation as the app shows it, built up from transcript pages: the latest page
 * first, new items as they're written, older pages on request. Immutable; each change returns a
 * new one.
 */
data class Conversation(
    val items: List<TranscriptItem> = emptyList(),
    /** Where the earliest loaded page starts in the log; null until the first page arrives. */
    val start: Long? = null,
    /** Where to continue from. */
    val end: Long = 0,
    /** Tool results whose call is on a page not loaded yet. */
    private val results: Map<String, TranscriptItem> = emptyMap(),
) {
    val isLoaded: Boolean get() = start != null
    val hasEarlier: Boolean get() = (start ?: 0) > 0

    companion object {
        /** The latest page, in place of everything loaded so far. */
        fun of(page: Transcript): Conversation =
            Conversation(start = page.start, end = page.end).merge(page.items, atFront = false)

        /** A later copy replaces an item; a bare result fills in its call. */
        private fun combine(old: TranscriptItem, new: TranscriptItem): TranscriptItem =
            if (new.text != null) new else old.copy(output = new.output, done = new.done, failed = new.failed)
    }

    /** A page asked for after [end]. One that doesn't start there skipped some, so it replaces. */
    fun append(page: Transcript, askedAfter: Long): Conversation =
        if (page.start != askedAfter) of(page) else copy(end = page.end).merge(page.items, atFront = false)

    /** A page asked for before [start]. */
    fun prepend(page: Transcript): Conversation = copy(start = page.start).merge(page.items, atFront = true)

    private fun merge(page: List<TranscriptItem>, atFront: Boolean): Conversation {
        val items = items.toMutableList()
        val results = results.toMutableMap()
        val added = mutableListOf<TranscriptItem>()
        for (item in page) {
            val i = items.indexOfFirst { it.id == item.id }
            val j = added.indexOfFirst { it.id == item.id }
            when {
                i >= 0 -> items[i] = combine(items[i], item)
                j >= 0 -> added[j] = combine(added[j], item)
                item.text == null -> results[item.id] = item
                else -> added += results.remove(item.id)?.let { combine(item, it) } ?: item
            }
        }
        return copy(items = if (atFront) added + items else items + added, results = results)
    }
}
