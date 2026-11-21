package com.digitaltracer.switchboard

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.PorterDuff
import android.graphics.PorterDuffXfermode
import android.graphics.RadialGradient
import android.graphics.Rect
import android.graphics.RectF
import android.graphics.Shader
import android.os.SystemClock
import android.view.View
import android.widget.RemoteViews
import com.digitaltracer.switchboard.core.LiveBoard
import com.digitaltracer.switchboard.core.LiveLayout
import com.digitaltracer.switchboard.core.LiveLines
import com.digitaltracer.switchboard.core.LiveMood

/**
 * The ongoing notification: the iPhone card's three lines beside a scene drawn here from bundled
 * bodies. It's built on the phone from the board the app fetched, so it can name agents; the
 * push that wakes it carries nothing.
 */
object LiveNotification {
    private const val CHANNEL = "live"
    private const val ID = 0x5B11
    /** The hub refreshes a quiet board every 45 minutes; one older than this is gone. */
    private const val TIMEOUT_MS = 65 * 60 * 1000L
    /** Scene units are the card's points; the bitmap is drawn at this many pixels per unit. */
    private const val SCALE = 2.5f
    private const val SCENE_W = 184

    fun channel(context: Context) {
        context.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, "Your agents", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Who needs you, who's running and who's idle, kept up to date."
                setShowBadge(false)
            },
        )
    }

    fun show(context: Context, board: LiveBoard, nowSecs: Long = System.currentTimeMillis() / 1000) {
        val manager = context.getSystemService(NotificationManager::class.java)
        if (board.isEmpty) {
            manager.cancel(ID)
            return
        }
        channel(context)
        val mood = LiveMood.of(board, nowSecs)
        val lines = LiveLines.of(board, mood)
        val scene = scene(context, board, mood)
        val open = PendingIntent.getActivity(
            context, ID, Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val n = Notification.Builder(context, CHANNEL)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setStyle(Notification.DecoratedCustomViewStyle())
            .setCustomContentView(views(context, R.layout.live_card_small, lines, scene, nowSecs, counts = false))
            .setCustomBigContentView(views(context, R.layout.live_card, lines, scene, nowSecs, counts = true))
            .setContentTitle(lines.title)
            .setContentText(lines.happened.trimEnd(' ', '·'))
            .setContentIntent(open)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(Notification.CATEGORY_STATUS)
            .setTimeoutAfter(TIMEOUT_MS)
            .build()
        manager.notify(ID, n)
    }

    fun showing(context: Context) = context.getSystemService(NotificationManager::class.java).activeNotifications.any { it.id == ID }

    fun cancel(context: Context) {
        context.getSystemService(NotificationManager::class.java).cancel(ID)
    }

    private fun views(
        context: Context,
        layout: Int,
        lines: LiveLines,
        scene: Bitmap,
        nowSecs: Long,
        counts: Boolean,
    ): RemoteViews {
        val v = RemoteViews(context.packageName, layout)
        v.setImageViewBitmap(R.id.live_scene, scene)
        v.setTextViewText(R.id.live_title, lines.title)
        v.setTextViewText(R.id.live_happened, lines.happened)
        v.setTextColor(R.id.live_happened, colour(lines.tone))
        val since = lines.waitingSince
        if (since != null) {
            val base = SystemClock.elapsedRealtime() - (nowSecs - since).coerceAtLeast(0) * 1000
            v.setChronometer(R.id.live_clock, base, null, true)
            v.setViewVisibility(R.id.live_clock, View.VISIBLE)
        } else {
            v.setViewVisibility(R.id.live_clock, View.GONE)
        }
        if (!counts) return v
        val lead = lines.lead
        if (lead == null) {
            v.setViewVisibility(R.id.live_lead, View.GONE)
        } else {
            v.setViewVisibility(R.id.live_lead, View.VISIBLE)
            v.setImageViewResource(R.id.live_lead, drawable(lead))
        }
        val slots = listOf(R.id.live_c1 to R.id.live_t1, R.id.live_c2 to R.id.live_t2, R.id.live_c3 to R.id.live_t3)
        slots.forEachIndexed { i, (icon, text) ->
            val c = lines.counts.getOrNull(i)
            v.setViewVisibility(icon, if (c == null) View.GONE else View.VISIBLE)
            v.setViewVisibility(text, if (c == null) View.GONE else View.VISIBLE)
            if (c != null) {
                v.setImageViewResource(icon, drawable(c.first))
                v.setTextViewText(text, c.second)
                v.setTextColor(text, if (i == 0 && lines.moreTone) colour(LiveLines.Tone.Amber) else Color.rgb(185, 190, 196))
            }
        }
        return v
    }

    private fun colour(t: LiveLines.Tone) = when (t) {
        LiveLines.Tone.Amber -> Color.rgb(255, 179, 127)
        LiveLines.Tone.Red -> Color.rgb(255, 154, 144)
        LiveLines.Tone.Green -> Color.rgb(111, 211, 160)
        LiveLines.Tone.Muted -> Color.rgb(169, 174, 180)
    }

    private fun drawable(b: LiveLines.Body) = when (b) {
        LiveLines.Body.Sun -> R.drawable.live_sun
        LiveLines.Body.RedSun -> R.drawable.live_sun_red
        LiveLines.Body.Finished -> R.drawable.live_finished
        LiveLines.Body.Planet -> R.drawable.live_jupiter
        LiveLines.Body.Moon -> R.drawable.live_moon
    }

    private fun texture(name: String) = when (name) {
        "saturn" -> R.drawable.live_saturn
        "earth" -> R.drawable.live_earth
        "mars" -> R.drawable.live_mars
        "neptune" -> R.drawable.live_neptune
        "venus" -> R.drawable.live_venus
        else -> R.drawable.live_jupiter
    }

    /** The system on the left of the card, as the iPhone lays it out. */
    fun scene(context: Context, board: LiveBoard, mood: LiveMood): Bitmap {
        val bmp = Bitmap.createBitmap((SCENE_W * SCALE).toInt(), (150 * SCALE).toInt(), Bitmap.Config.ARGB_8888)
        val c = Canvas(bmp)
        c.drawColor(Color.rgb(5, 7, 10))
        c.scale(SCALE, SCALE)
        val res = context.resources
        fun img(id: Int) = BitmapFactory.decodeResource(res, id)
        val paint = Paint(Paint.ANTI_ALIAS_FLAG or Paint.FILTER_BITMAP_FLAG)
        img(R.drawable.live_stars).let { s ->
            val src = Rect(0, 0, (s.width * SCENE_W / 420f).toInt(), s.height)
            c.drawBitmap(s, src, RectF(0f, 0f, SCENE_W.toFloat(), 150f), paint)
        }
        val layout = LiveLayout.of(board, mood)
        val (cx, cy) = layout.centre.first.toFloat() to layout.centre.second.toFloat()
        val glow = when (mood) {
            is LiveMood.NeedsYou -> Color.rgb(255, 140, 56)
            is LiveMood.Error -> Color.rgb(255, 64, 41)
            is LiveMood.Finished -> Color.rgb(77, 235, 148)
            LiveMood.Calm -> null
        }
        if (glow != null) {
            val p = Paint().apply {
                shader = RadialGradient(cx, cy, 150f, Color.argb(56, Color.red(glow), Color.green(glow), Color.blue(glow)),
                    Color.TRANSPARENT, Shader.TileMode.CLAMP)
            }
            c.drawRect(0f, 0f, SCENE_W.toFloat(), 150f, p)
        }
        // Orbits, faded out before the text column.
        val layer = c.saveLayer(0f, 0f, SCENE_W.toFloat(), 150f, null)
        val ring = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            style = Paint.Style.STROKE
            strokeWidth = 0.6f
            color = Color.argb(48, 184, 204, 255)
        }
        for (a in layout.rings) {
            c.save()
            c.translate(cx, cy)
            c.rotate(-10f)
            val r = a.toFloat()
            c.drawOval(-r, -r * 0.5f, r, r * 0.5f, ring)
            c.restore()
        }
        c.drawRect(0f, 0f, SCENE_W.toFloat(), 150f, Paint().apply {
            shader = LinearGradient(SCENE_W - 44f, 0f, SCENE_W.toFloat(), 0f, Color.BLACK, Color.TRANSPARENT, Shader.TileMode.CLAMP)
            xfermode = PorterDuffXfermode(PorterDuff.Mode.DST_IN)
        })
        c.restoreToCount(layer)
        fun sprite(id: Int, x: Float, y: Float, size: Float) =
            c.drawBitmap(img(id), null, RectF(x - size / 2, y - size / 2, x + size / 2, y + size / 2), paint)
        val label = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            textSize = 10f
            color = Color.rgb(201, 205, 210)
            textAlign = Paint.Align.CENTER
            setShadowLayer(1.5f, 0f, 1f, Color.argb(230, 0, 0, 0))
        }
        for (b in layout.bodies) {
            val (x, y) = b.x.toFloat() to b.y.toFloat()
            if (b.texture == null) {
                sprite(R.drawable.live_moon, x, y, (b.radius / 0.32).toFloat())
            } else {
                sprite(texture(b.texture!!), x, y, (b.radius * 6).toFloat())
            }
            b.label?.let { c.drawText(it, x, y + b.radius.toFloat() + 12f, label) }
        }
        when {
            mood is LiveMood.NeedsYou && mood.others > 0 -> {
                sprite(R.drawable.live_sun, 44f, 80f, 27f * 6)
                val second = board.waiting.getOrNull(1)
                sprite(if (second?.error == true) R.drawable.live_sun_red else R.drawable.live_sun, 110f, 36f, 15f * 6)
                if (second != null) {
                    label.color = Color.rgb(255, 179, 127)
                    c.drawText(LiveLayout.short(second.name), 112f, 64f, label)
                }
            }
            mood is LiveMood.NeedsYou -> sprite(R.drawable.live_sun, cx, cy, 29f * 6)
            mood is LiveMood.Error -> sprite(R.drawable.live_sun_red, cx, cy, 27f * 6)
            mood is LiveMood.Finished -> sprite(R.drawable.live_finished, cx, cy, 16f * 6)
            else -> {}
        }
        return bmp
    }
}
