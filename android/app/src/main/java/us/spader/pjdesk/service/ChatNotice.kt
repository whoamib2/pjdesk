package us.spader.pjdesk.service

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat
import us.spader.pjdesk.MainActivity
import us.spader.pjdesk.R

/**
 * High-importance notice when the person at the other computer sends a chat
 * line. Tapping it opens PJDesk. It does not end the session.
 */
object ChatNotice {
    private const val ID = 11
    private const val CHANNEL = "pjdesk-chat"

    fun show(context: Context, name: String, text: String) {
        val app = context.applicationContext
        val manager = app.getSystemService(NotificationManager::class.java) ?: return
        val channel = NotificationChannel(CHANNEL, "Session chat", NotificationManager.IMPORTANCE_HIGH)
        channel.enableLights(true)
        channel.enableVibration(true)
        manager.createNotificationChannel(channel)
        val open = PendingIntent.getActivity(
            app,
            11,
            Intent(app, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val who = name.ifBlank { "The other computer" }
        val notification = NotificationCompat.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat)
            .setContentTitle(who)
            .setContentText(text)
            .setContentIntent(open)
            .setAutoCancel(true)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .build()
        manager.notify(ID, notification)
    }

    fun hide(context: Context) {
        context.applicationContext.getSystemService(NotificationManager::class.java)?.cancel(ID)
    }
}
