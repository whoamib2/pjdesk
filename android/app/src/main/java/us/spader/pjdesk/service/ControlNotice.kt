package us.spader.pjdesk.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.os.Build
import androidx.core.app.NotificationCompat
import us.spader.pjdesk.R
import us.spader.pjdesk.a11y.PjAccessibilityService

/**
 * Stays up while remote control cannot run. An update turns the accessibility
 * service off. PJDesk does not write the secure setting that would turn it
 * back on. Tapping this opens that service's own page.
 */
object ControlNotice {
    const val TEXT = "Control is off, tap to turn on"
    private const val CHANNEL = "pjdesk-control"
    private const val NOTIF = 10

    fun sync(context: Context) {
        val app = context.applicationContext
        if (PjAccessibilityService.ready(app)) cancel(app) else show(app)
    }

    fun show(context: Context) {
        val app = context.applicationContext
        val manager = app.getSystemService(NotificationManager::class.java) ?: return
        if (Build.VERSION.SDK_INT >= 26) {
            val channel = NotificationChannel(CHANNEL, app.getString(R.string.channel_control), NotificationManager.IMPORTANCE_DEFAULT)
            channel.description = TEXT
            channel.lockscreenVisibility = Notification.VISIBILITY_PUBLIC
            manager.createNotificationChannel(channel)
        }
        val open = PendingIntent.getActivity(
            app,
            12,
            PjAccessibilityService.detailsIntent(app),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat)
            .setContentTitle(TEXT)
            .setContentText(TEXT)
            .setContentIntent(open)
            .setPriority(NotificationCompat.PRIORITY_DEFAULT)
            .setCategory(NotificationCompat.CATEGORY_STATUS)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .setAutoCancel(false)
            .build()
        manager.notify(NOTIF, notification)
    }

    fun cancel(context: Context) {
        context.applicationContext.getSystemService(NotificationManager::class.java)?.cancel(NOTIF)
    }
}
