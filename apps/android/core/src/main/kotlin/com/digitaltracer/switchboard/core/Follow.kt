package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.Output
import com.digitaltracer.switchboard.api.models.Transcript
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.withTimeoutOrNull

/** Whose conversation: a Switchboard session's, or an agent started in Herdr. */
sealed interface ConversationSource {
    data class Session(val id: String) : ConversationSource
    data class Agent(val machine: String, val name: String) : ConversationSource
}

/**
 * Following a conversation while it's on screen: the transcript as a long-poll the hub holds
 * until there's more, and the agent's screen once a second while it's shown.
 */
object Follow {
    /** How long the hub is asked to hold a transcript request. */
    const val WAIT_MS = 20_000L

    /** The least time from one request's start to the next: a screen tick, and the pace of a hub too old to wait. */
    const val EVERY_MS = 1_000L

    /** After a failed request. */
    const val RETRY_MS = 3_000L

    /** The longest an unavailable conversation waits before it's asked for again. */
    const val UNAVAILABLE_MAX_MS = 60_000L

    /** Whether a conversation's screen is read: only where it's drawn, while wanted or standing in for a missing conversation. */
    fun readsScreen(draws: Boolean, wanted: Boolean, unavailable: Boolean): Boolean = draws && (wanted || unavailable)

    /**
     * A page of the conversation. With [wait], one that continues from [after] waits for more;
     * only following asks for that, since a refresh someone is waiting on mustn't hang.
     */
    suspend fun page(hub: Hub, source: ConversationSource, after: Long?, before: Long?, wait: Boolean = false): Transcript {
        val wait = if (wait && after != null && before == null) WAIT_MS else null
        return when (source) {
            is ConversationSource.Session -> hub.transcript(source.id, after, before, wait)
            is ConversationSource.Agent -> hub.agentTranscript(source.machine, source.name, after, before, wait)
        }
    }

    /** Only the screen: reading further back would scroll the agent's terminal on its machine. */
    suspend fun screen(hub: Hub, source: ConversationSource): Output = when (source) {
        is ConversationSource.Session -> hub.output(source.id, visible = true, ansi = true)
        is ConversationSource.Agent -> hub.agentOutput(source.machine, source.name, visible = true, ansi = true)
    }

    /**
     * Reads the screen while [shown], one read at a time and at most once every [everyMs]. While
     * it's hidden nothing runs until [bell] rings; a ring also brings the next read forward, as
     * when the Screen tab is opened.
     */
    suspend fun screens(
        bell: Bell,
        shown: () -> Boolean,
        going: () -> Boolean = { true },
        everyMs: Long = EVERY_MS,
        retryMs: Long = RETRY_MS,
        now: () -> Long = { System.nanoTime() / 1_000_000 },
        read: suspend () -> Boolean,
    ) {
        while (currentCoroutineContext().isActive && going()) {
            if (!shown()) {
                bell.await()
                continue
            }
            val start = now()
            val rest = (if (read()) everyMs else retryMs) - (now() - start)
            if (rest > 0) bell.await(rest)
        }
    }

    /**
     * Runs [round] until cancelled or [going] turns false, one at a time, each starting at least
     * [everyMs] after the last began, or [retryMs] after one that failed. [round] returns false on failure.
     */
    suspend fun repeating(
        everyMs: Long = EVERY_MS,
        retryMs: Long = RETRY_MS,
        going: () -> Boolean = { true },
        now: () -> Long = { System.nanoTime() / 1_000_000 },
        round: suspend () -> Boolean,
    ) {
        while (currentCoroutineContext().isActive && going()) {
            val start = now()
            val ok = round()
            val rest = (if (ok) everyMs else retryMs) - (now() - start)
            if (rest > 0) delay(rest)
        }
    }
}

/**
 * How long an unavailable conversation waits before it's asked for again: [Follow.RETRY_MS],
 * doubling each time to [Follow.UNAVAILABLE_MAX_MS], and back to the usual pace once it's there.
 */
class UnavailableBackoff {
    private var times = 0

    /** The wait after a round beyond the usual retry, given whether the conversation was unavailable. */
    fun extraMs(unavailable: Boolean): Long {
        if (!unavailable) {
            times = 0
            return 0
        }
        times = (times + 1).coerceAtMost(16)
        return (Follow.RETRY_MS shl (times - 1)).coerceAtMost(Follow.UNAVAILABLE_MAX_MS) - Follow.RETRY_MS
    }
}

/** Wakes one waiting loop. A ring while nothing waits is kept for the next wait. */
class Bell {
    private val rings = Channel<Unit>(Channel.CONFLATED)

    fun ring() {
        rings.trySend(Unit)
    }

    /** Returns when rung, or after [limitMs] if there is one. */
    suspend fun await(limitMs: Long? = null) {
        if (limitMs == null) rings.receive() else withTimeoutOrNull(limitMs) { rings.receive() }
    }
}
