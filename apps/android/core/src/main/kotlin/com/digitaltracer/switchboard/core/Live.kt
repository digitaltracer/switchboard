package com.digitaltracer.switchboard.core

import com.digitaltracer.switchboard.api.models.LiveBoardView
import kotlin.math.cos
import kotlin.math.sin

/** What the ongoing notification shows, as the hub's `/live` builds it; names are masked. */
data class LiveBoard(
    val at: Long,
    val waiting: List<Waiting> = emptyList(),
    val more: Int = 0,
    val running: List<String> = emptyList(),
    val runningMore: Int = 0,
    val idle: Int = 0,
    val finished: String? = null,
    val finishedAt: Long? = null,
) {
    /** An agent that needs the owner, or stopped on an error. Times are Unix seconds. */
    data class Waiting(val name: String, val space: String?, val since: Long, val error: Boolean)

    val runningCount get() = running.size + runningMore
    val waitingCount get() = waiting.size + more
    val isEmpty get() = waiting.isEmpty() && runningCount == 0 && idle == 0
    /** Nothing working, waiting or just finished, as the hub's `Board::is_quiet` says. */
    val isQuiet get() = waitingCount == 0 && runningCount == 0 && finished == null

    companion object {
        fun of(v: LiveBoardView) = LiveBoard(
            at = v.at,
            waiting = v.waiting.map { Waiting(it.name, it.space, it.since, it.error) },
            more = v.more,
            running = v.running,
            runningMore = v.runningMore,
            idle = v.idle,
            finished = v.finished,
            finishedAt = v.finishedAt,
        )
    }
}

/** When the ongoing notification shows, chosen per phone and kept on the hub. */
enum class LiveShowMode(val wire: String) {
    /** While something works or waits; it ends fifteen minutes after everything is idle. */
    Working("working"),
    /** Always, with "All quiet" when nothing runs. */
    Always("always");

    enum class Step { Show, End, None }

    /** What to do for this board; [quietSince] is when it went quiet (Unix ms). */
    fun decide(board: LiveBoard, showing: Boolean, quietSince: Long?, nowMs: Long): Step = when {
        this == Always || !board.isQuiet -> Step.Show
        !showing -> Step.None
        quietSince != null && nowMs - quietSince >= QUIET_END_MS -> Step.End
        else -> Step.Show
    }

    companion object {
        const val QUIET_END_MS = 15 * 60 * 1000L
        fun of(wire: String?) = entries.firstOrNull { it.wire == wire } ?: Working
    }
}

/** Which of the card's states a board is in. */
sealed interface LiveMood {
    data object Calm : LiveMood
    data class NeedsYou(val who: LiveBoard.Waiting, val others: Int) : LiveMood
    data class Error(val who: LiveBoard.Waiting, val others: Int) : LiveMood
    data class Finished(val name: String) : LiveMood

    companion object {
        /** A finished agent shows for about a minute, then the card returns to calm. */
        const val FINISHED_FOR_SECS = 60L

        fun of(b: LiveBoard, nowSecs: Long): LiveMood {
            val first = b.waiting.firstOrNull()
            return when {
                first != null && first.error -> Error(first, b.waitingCount - 1)
                first != null -> NeedsYou(first, b.waitingCount - 1)
                b.finished != null && b.finishedAt != null && nowSecs - b.finishedAt < FINISHED_FOR_SECS -> Finished(b.finished)
                else -> Calm
            }
        }
    }
}

/** The card's three lines: a name, what happened, the counts. Every state has the same three. */
data class LiveLines(
    /** None beside "All quiet": nothing is running, so no planet. */
    val lead: Body?,
    val title: String,
    val happened: String,
    val tone: Tone,
    /** Unix seconds the wait started, for a ticking clock after [happened]. */
    val waitingSince: Long?,
    val counts: List<Pair<Body, String>>,
    /** Whether the counts line opens with "+N more" in the needs-you colour. */
    val moreTone: Boolean,
) {
    enum class Tone { Amber, Red, Green, Muted }
    enum class Body { Sun, RedSun, Finished, Planet, Moon }

    companion object {
        fun of(b: LiveBoard, mood: LiveMood): LiveLines = when (mood) {
            is LiveMood.NeedsYou -> LiveLines(
                Body.Sun, mood.who.name, "needs you · ", Tone.Amber, mood.who.since,
                if (mood.others > 0) {
                    listOf(Body.Sun to "+${mood.others} more", Body.Planet to "${b.runningCount}", Body.Moon to "${b.idle}")
                } else {
                    listOf(Body.Planet to "${b.runningCount} running", Body.Moon to "${b.idle} idle")
                },
                mood.others > 0,
            )
            is LiveMood.Error -> LiveLines(
                Body.RedSun, mood.who.name, "stopped on an error", Tone.Red, null,
                listOf(Body.Planet to "${b.runningCount} running", Body.Moon to "${b.idle} idle"), false,
            )
            is LiveMood.Finished -> LiveLines(
                Body.Finished, mood.name, "finished just now", Tone.Green, null,
                listOf(Body.Planet to "${b.runningCount} running", Body.Moon to "${b.idle} idle"), false,
            )
            LiveMood.Calm -> LiveLines(
                if (b.runningCount == 0) null else Body.Planet, if (b.runningCount == 0) "All quiet" else "${b.runningCount} running",
                "nothing needs you", Tone.Muted, null, listOf(Body.Moon to "${b.idle} idle"), false,
            )
        }
    }
}

/**
 * Where each body of the scene goes, as the iPhone's card lays it out: planets on tilted orbits
 * around the lead, moons outside. Points are on a 150-unit-tall scene.
 */
data class LiveLayout(val centre: Pair<Double, Double>, val rings: List<Double>, val bodies: List<Body>) {
    data class Body(val texture: String?, val x: Double, val y: Double, val radius: Double, val label: String?)

    companion object {
        private const val TILT = 0.5
        private val ROT = Math.toRadians(-10.0)
        val TEXTURES = listOf("jupiter", "saturn", "earth", "mars", "neptune", "venus")
        private val RADII = mapOf("jupiter" to 8.0, "saturn" to 6.5, "earth" to 6.0, "mars" to 5.0, "neptune" to 7.0, "venus" to 5.5)
        private val LEAD = listOf(0 to 35.0, 1 to -48.0, 2 to 8.0, 1 to 80.0)
        private val TWO = listOf(1 to 30.0, 2 to -22.0, 2 to 62.0)
        // Bodies and labels stay left of the text column, so a moon never reads as a legend icon.
        private val CALM = listOf(0 to -80.0, 0 to 125.0, 1 to -140.0, 1 to -30.0, 1 to 62.0)
        private val LEAD_MOONS = listOf(3 to -35.0, 3 to 52.0, 3 to 78.0, 3 to -56.0, 2 to 40.0)
        private val TWO_MOONS = listOf(2 to 95.0, 2 to -60.0, 3 to 110.0, 3 to -30.0)
        private val CALM_MOONS = listOf(2 to -45.0, 2 to 100.0, 2 to -100.0, 2 to 140.0)

        fun point(c: Pair<Double, Double>, a: Double, deg: Double): Pair<Double, Double> {
            val t = Math.toRadians(deg)
            val x = a * cos(t)
            val y = a * TILT * sin(t)
            return (c.first + x * cos(ROT) - y * sin(ROT)) to (c.second + x * sin(ROT) + y * cos(ROT))
        }

        /** The same agent keeps its planet; within one scene, planets don't repeat while others are free. */
        fun texture(name: String, used: Set<String> = emptySet()): String {
            var h = 5381u
            for (b in name.encodeToByteArray()) h = h * 33u + b.toUByte().toUInt()
            val start = (h % TEXTURES.size.toUInt()).toInt()
            val order = TEXTURES.indices.map { TEXTURES[(start + it) % TEXTURES.size] }
            return order.firstOrNull { it !in used } ?: order[0]
        }

        fun short(name: String) = if (name.length <= 12) name else name.take(11) + "…"

        fun of(b: LiveBoard, mood: LiveMood): LiveLayout {
            val (centre, rings, slotsAndMoons) = when {
                mood is LiveMood.Calm -> Triple(86.0 to 72.0, listOf(46.0, 72.0, 104.0), CALM to CALM_MOONS)
                mood is LiveMood.NeedsYou && mood.others > 0 -> Triple(62.0 to 68.0, listOf(74.0, 98.0, 122.0, 140.0), TWO to TWO_MOONS)
                else -> Triple(50.0 to 75.0, listOf(66.0, 90.0, 114.0, 140.0), LEAD to LEAD_MOONS)
            }
            val (slots, moons) = slotsAndMoons
            val names = b.running.filter { !(mood is LiveMood.Finished && it == mood.name) }
            val used = mutableSetOf<String>()
            val out = mutableListOf<Body>()
            names.take(slots.size).forEachIndexed { i, name ->
                val (ring, deg) = slots[i]
                val tex = texture(name, used).also { used += it }
                val (x, y) = point(centre, rings[ring], deg)
                out += Body(tex, x, y, RADII.getValue(tex), short(name))
            }
            val idle = minOf(b.idle, moons.size)
            val extra = maxOf(0, b.runningCount - minOf(names.size, slots.size))
            moons.drop(idle).take(extra).forEachIndexed { i, (ring, deg) ->
                val (x, y) = point(centre, rings[minOf(ring, rings.size - 1)], deg)
                out += Body(TEXTURES[i % TEXTURES.size], x, y, 4.0, null)
            }
            moons.take(idle).forEachIndexed { i, (ring, deg) ->
                val (x, y) = point(centre, rings[minOf(ring, rings.size - 1)], deg)
                out += Body(null, x, y, if (i % 2 == 0) 3.2 else 2.8, null)
            }
            return LiveLayout(centre, rings, out)
        }
    }
}
