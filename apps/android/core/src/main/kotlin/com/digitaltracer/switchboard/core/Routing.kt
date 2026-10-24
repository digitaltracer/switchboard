package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.MessagePreview
import com.digitaltracer.switchboard.api.models.Session
import com.digitaltracer.switchboard.api.models.TargetView

/** One place a message can go, as the review sheet lists it under "Goes to". */
data class RouteOption(
    val id: String,
    val title: String,
    val detail: String,
    /** Starts a new session rather than continuing one. */
    val isNew: Boolean,
    val action: Action,
    /** What the send button says for this option. */
    val verb: String,
) {
    sealed interface Action {
        /** Send as is: the hub routes it the way its preview said. */
        data object AsPreviewed : Action
        data class Start(val projectId: String) : Action
        data class Send(val target: TargetView) : Action
    }
}

/**
 * The hub's own choice first, then the plausible alternatives: a new session in the project of
 * what it would continue, or the running sessions of the project it would start in.
 */
object RouteOptions {
    data class Result(
        val options: List<RouteOption>,
        /** The option picked until the user picks another; none when the hub has to ask. */
        val suggested: String?,
    )

    private const val MAX_ALTERNATIVES = 3

    fun build(preview: MessagePreview, sessions: List<Session>, context: TargetView?): Result {
        val live = sessions.filter { it.lifecycle in setOf("running", "starting") }
        fun project(id: String) = sessions.firstOrNull { it.id == id }?.projectId
        fun status(t: TargetView) = t.sessionId?.let { id -> live.firstOrNull { it.id == id }?.agentStatus }
        var top: RouteOption? = null
        var rest = mutableListOf<RouteOption>()
        when (preview.outcome) {
            "start" -> {
                val p = preview.projectId.orEmpty()
                top = startOption(p, preview.machineId, RouteOption.Action.AsPreviewed)
                rest += live.filter { it.projectId == p }.map { sendOption(sessionTarget(it), it.agentStatus) }
            }
            "send", "select" -> preview.target?.let { t ->
                top = sendOption(t, status(t), RouteOption.Action.AsPreviewed)
                t.sessionId?.let(::project)?.let { p -> rest += startOption(p, t.machineId, RouteOption.Action.Start(p)) }
            }
            "choose_project" -> {
                rest += preview.options.mapNotNull { o -> o.projectId?.let { startOption(it, null, RouteOption.Action.Start(it)) } }
                rest += live.take(MAX_ALTERNATIVES).map { sendOption(sessionTarget(it), it.agentStatus) }
            }
            "choose_target" -> rest += preview.options.mapNotNull { o -> o.target?.let { sendOption(it, null) } }
        }
        if (context != null && top?.id != sendId(context)) rest.add(0, sendOption(context, status(context)))
        val seen = mutableSetOf<String>().apply { top?.let { add(it.id) } }
        rest = rest.filter { seen.add(it.id) }.toMutableList()
        if (preview.outcome in setOf("start", "send", "select")) rest = rest.take(MAX_ALTERNATIVES).toMutableList()
        return Result(listOfNotNull(top) + rest, top?.id)
    }

    private fun sendId(t: TargetView) = "send:" + (t.sessionId ?: "${t.machineId}/${t.agent ?: t.label}")

    private fun startOption(project: String, machine: String?, action: RouteOption.Action) = RouteOption(
        id = "start:$project",
        title = "New task in $project",
        detail = listOfNotNull(machine?.let { "On $it" }, "its own branch").joinToString(" · "),
        isNew = true,
        action = action,
        verb = "Start a task in $project",
    )

    private fun sendOption(t: TargetView, status: String?, action: RouteOption.Action? = null): RouteOption {
        val what = if (t.kind == "agent") "Your Herdr session" else status?.replaceFirstChar { it.uppercase() } ?: "Running"
        return RouteOption(
            id = sendId(t),
            title = "Continue ${t.label}",
            detail = "$what · ${t.machineId}",
            isNew = false,
            action = action ?: RouteOption.Action.Send(t),
            verb = "Send to ${t.label}",
        )
    }
}
