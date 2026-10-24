package us.spader.pjdesk.service

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationCompat
import us.spader.pjdesk.MainActivity
import us.spader.pjdesk.PjApp
import us.spader.pjdesk.R

/**
 * Ongoing notification for a session this phone is controlling.
 * Disconnect ends that session only. The host registration stays up.
 */
object SessionNotice {
    const val ACTION_DISCONNECT = "us.spader.pjdesk.SESSION_DISCONNECT"
    private const val ID = 9
    private const val CHANNEL = "pjdesk-session"

    fun show(context: Context, peer: String) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        val channel = NotificationChannel(
            CHANNEL,
            context.getString(R.string.channel_session),
            NotificationManager.IMPORTANCE_LOW,
        )
        manager.createNotificationChannel(channel)
        val open = PendingIntent.getActivity(
            context,
            3,
            Intent(context, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val disconnect = PendingIntent.getBroadcast(
            context,
            4,
            Intent(context, SessionStopReceiver::class.java).setAction(ACTION_DISCONNECT),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val who = peer.ifBlank { "a computer" }
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat)
            .setContentTitle(context.getString(R.string.session_controlling, who))
            .setContentText(context.getString(R.string.session_controlling_body))
            .setOngoing(true)
            .setContentIntent(open)
            .addAction(0, context.getString(R.string.session_disconnect), disconnect)
            .build()
        manager.notify(ID, notification)
    }

    fun hide(context: Context) {
        context.getSystemService(NotificationManager::class.java)?.cancel(ID)
    }
}

class SessionStopReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent?) {
        if (intent?.action != SessionNotice.ACTION_DISCONNECT) return
        (context.applicationContext as PjApp).engine.stopView("disconnected")
        SessionNotice.hide(context)
    }
}
