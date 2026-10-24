package com.digitaltracer.switchboard.core

import kotlin.coroutines.cancellation.CancellationException

/** Time left on an approval, as `m:ss`. Unanswered at zero means denied. */
object Countdown {
    fun text(deadline: Long, now: Long): String {
        val seconds = maxOf(0, (deadline - now) / 1000)
        return "%d:%02d".format(seconds / 60, seconds % 60)
    }

    fun nowMs(): Long = System.currentTimeMillis()
}

/**
 * A start the phone sent. A lost answer isn't a failure: the hub may have started it, so the
 * app looks the request up by its id until the hub says.
 */
sealed interface StartState {
    data class Started(val sessionId: String, val alreadyRunning: Boolean) : StartState

    /** No answer, or the hub is still finding out; reconcile by request id. */
    data class Unknown(val requestId: String) : StartState

    data class Failed(val message: String) : StartState
}

object StartFlow {
    suspend fun run(
        hub: Hub, project: String, prompt: String, machine: String?, agent: String?, notify: Boolean, requestId: String,
    ): StartState = try {
        val r = hub.start(project, prompt, machine, agent, notify, requestId)
        StartState.Started(r.sessionId, alreadyRunning = r.outcome == "deduplicated")
    } catch (e: HubError) {
        if (e.outcomeUnknown) StartState.Unknown(requestId) else StartState.Failed(e.message.orEmpty())
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        StartState.Failed(e.message ?: e.javaClass.simpleName)
    }

    /**
     * One look at the hub's record of the request. `attempt` counts from 0: a request sent a
     * moment ago may still be on its way, so "never reached" only counts from the third look.
     */
    suspend fun reconcile(hub: Hub, requestId: String, attempt: Int = 2): StartState = try {
        val op = hub.operation(requestId)
        when (op.state) {
            "done" -> op.sessionId?.let { StartState.Started(it, alreadyRunning = false) }
                ?: StartState.Failed("The hub has no session for it.")
            "failed" -> StartState.Failed(op.errorMessage ?: "The start failed.")
            else -> StartState.Unknown(requestId)
        }
    } catch (e: HubError) {
        // The request never reached the hub, so retrying with the same id is safe.
        if (e.code == "unknown_operation" && attempt >= 2) {
            StartState.Failed("The start never reached the hub. Try again.")
        } else {
            StartState.Unknown(requestId)
        }
    } catch (e: CancellationException) {
        throw e
    } catch (_: Exception) {
        StartState.Unknown(requestId)
    }
}
