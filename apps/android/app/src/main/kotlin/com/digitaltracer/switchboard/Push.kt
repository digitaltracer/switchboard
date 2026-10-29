package com.digitaltracer.switchboard

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import com.digitaltracer.switchboard.core.PushTarget
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage

/**
 * Push through Firebase Cloud Messaging. The Firebase project comes from the build's config, not
 * a google-services file; without one, push stays off. Notifications carry the hub's generic
 * text and ids only.
 */
object Push {
    private const val CHANNEL = "agents"
    const val EXTRA_PREFIX = "switchboard.push."

    /** Starts Firebase if this build names a project; false when push is off. */
    fun start(context: Context): Boolean {
        if (BuildConfig.FIREBASE_APP_ID.isEmpty()) return false
        if (FirebaseApp.getApps(context).isEmpty()) {
            val options = FirebaseOptions.Builder()
                .setApplicationId(BuildConfig.FIREBASE_APP_ID)
                .setApiKey(BuildConfig.FIREBASE_API_KEY)
                .setProjectId(BuildConfig.FIREBASE_PROJECT_ID)
                .setGcmSenderId(BuildConfig.FIREBASE_SENDER_ID)
                .build()
            FirebaseApp.initializeApp(context, options)
        }
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Agents", NotificationManager.IMPORTANCE_HIGH),
        )
        return true
    }

    /** Hands the current token to [register] once Firebase has one. */
    fun token(register: (String) -> Unit) {
        FirebaseMessaging.getInstance().token.addOnSuccessListener { it?.let(register) }
    }

    fun show(context: Context, title: String, body: String, data: Map<String, String>, id: Int) {
        val open = Intent(context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
            data.forEach { (k, v) -> putExtra(EXTRA_PREFIX + k, v) }
        }
        val pending = PendingIntent.getActivity(
            context, id, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val n = android.app.Notification.Builder(context, CHANNEL)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(title)
            .setContentText(body)
            .setContentIntent(pending)
            .setAutoCancel(true)
            .build()
        context.getSystemService(NotificationManager::class.java).notify(id, n)
    }

    /** What a tapped notification points at: our own extras, or FCM's data when it showed it. */
    fun target(intent: Intent): PushTarget? {
        val extras = intent.extras ?: return null
        val data = extras.keySet().mapNotNull { k ->
            val key = k.removePrefix(EXTRA_PREFIX)
            extras.getString(k)?.let { key to it }
        }.toMap()
        return PushTarget.parse(data)
    }
}

class PushService : FirebaseMessagingService() {
    private val model get() = (application as SwitchboardApp).model

    override fun onNewToken(token: String) {
        model.registerPush(token)
    }

    override fun onMessageReceived(message: RemoteMessage) {
        val target = PushTarget.parse(message.data) ?: return
        val n = message.notification
        Push.show(
            this,
            n?.title ?: "Switchboard",
            n?.body ?: "",
            message.data,
            (target.attentionId ?: target.approvalId ?: target.sessionId ?: target.kind).hashCode(),
        )
    }
}
