package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.ShipButtonView
import com.digitaltracer.switchboard.api.models.ShipButtons as ShipButtonsView

/** A pull request named in an agent's reply. Switchboard never calls GitHub: this is all it knows. */
data class PullRequest(val url: String, val number: Int, val repo: String, val title: String?) {
    companion object {
        private val PATTERN = Regex(
            """https://(?:github\.com/([\w.-]+/[\w.-]+)/pull/(\d+)|gitlab\.com/([\w./-]+?)/-/merge_requests/(\d+)|bitbucket\.org/([\w.-]+/[\w.-]+)/pull-requests/(\d+))""",
        )
        private val LINK_TEXT = Regex("""\[([^\]\n]+)]\($""")

        /** The last pull request link in [text]. */
        fun find(text: String): PullRequest? {
            val m = PATTERN.findAll(text).lastOrNull() ?: return null
            val g = listOf(1, 3, 5).first { m.groups[it] != null }
            val title = LINK_TEXT.find(text.substring(0, m.range.first))?.groupValues?.get(1)?.takeIf { !it.startsWith("http") }
            return PullRequest(m.value, m.groupValues[g + 1].toInt(), m.groupValues[g], title)
        }
    }
}

/** A button that asks an agent to run one of the owner's skills: `/pr` for Claude Code, `$pr` for Codex. */
data class ShipButton(val skill: String, val label: String, val afterPr: Boolean = false) {
    constructor(v: ShipButtonView) : this(v.skill, v.label, v.afterPr ?: false)

    val view get() = ShipButtonView(skill, label, afterPr)
    val base get() = ShipButtons.base(skill)

    /** What the agent gets: the skill, then any instructions. */
    fun message(instructions: String = ""): String = instructions.trim().let { if (it.isEmpty()) skill else "$skill $it" }

    /** The same skill as this agent lists it, so a `/pr` button runs `$pr` on a Codex agent. */
    fun invocation(skills: List<String>): ShipButton {
        if (skill in skills) return this
        return skills.firstOrNull { ShipButtons.base(it) == base }?.let { copy(skill = it) } ?: this
    }

    /** Whether running it should end with a pull request, so its absence is worth saying. */
    val opensPr: Boolean
        get() {
            val words = base.lowercase().split(Regex("[^a-z]+"))
            return "pr" in words || "ship" in words || "pull" in base.lowercase()
        }
}

object ShipButtons {
    /** By base name. */
    private val DEFAULTS = listOf(
        ShipButton("pr", "Raise PR"), ShipButton("ship", "Ship"), ShipButton("commit", "Commit only"),
        ShipButton("code-review", "Review first"), ShipButton("triage-reviews", "Triage reviews", true),
        ShipButton("fix-ci", "Fix CI", true),
    )

    fun base(skill: String) = if (skill.firstOrNull() == '/' || skill.firstOrNull() == '$') skill.drop(1) else skill

    /** The project's own list, else the one for all projects. */
    fun effective(v: ShipButtonsView, project: String?): List<ShipButton> =
        (v.projects.firstOrNull { it.project == project }?.buttons ?: v.all).map(::ShipButton)

    /** Before the owner picks any: the usual ship skills this agent actually has, as it names them. */
    fun suggested(skills: List<String>): List<ShipButton> =
        DEFAULTS.mapNotNull { d -> skills.firstOrNull { base(it) == d.skill }?.let { ShipButton(it, d.label, d.afterPr) } }

    fun label(skill: String): String {
        val name = base(skill)
        DEFAULTS.firstOrNull { it.skill == name }?.let { return it.label }
        return name.substringAfterLast(':').replace('-', ' ').replace('_', ' ').replaceFirstChar { it.uppercase() }
    }

    fun afterPrByDefault(skill: String) = DEFAULTS.firstOrNull { it.skill == base(skill) }?.afterPr ?: false
}

/** What a ship skill came to, from the agent's latest reply once its turn ended. */
sealed interface ShipOutcome {
    data object Running : ShipOutcome
    data class Opened(val pr: PullRequest) : ShipOutcome
    /** It was meant to open one and didn't; the reply says why. */
    data object NoPullRequest : ShipOutcome
    data object Done : ShipOutcome

    companion object {
        fun of(sent: ShipButton, turnEnded: Boolean, reply: String?): ShipOutcome {
            reply?.let(PullRequest::find)?.let { return Opened(it) }
            if (!turnEnded) return Running
            return if (sent.opensPr) NoPullRequest else Done
        }
    }
}
