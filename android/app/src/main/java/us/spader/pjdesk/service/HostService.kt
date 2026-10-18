package us.spader.pjdesk.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.IBinder
import android.util.DisplayMetrics
import android.util.Log
import android.view.WindowManager
import androidx.core.app.NotificationCompat
import us.spader.pjdesk.MainActivity
import us.spader.pjdesk.PjApp
import us.spader.pjdesk.R
import us.spader.pjdesk.media.ScreenEncoder

/**
 * Foreground host. While this phone is only registered, the notification uses
 * the special-use type. While the screen is being shared it uses the media
 * projection type, which is the persistent "being controlled" indicator.
 * Stop ends the session and the capture.
 */
class HostService : Service() {
    private var projection: MediaProjection? = null
    private var encoder: ScreenEncoder? = null
    private var controlled = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                (application as PjApp).engine.stop()
                releaseCapture()
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
                return START_NOT_STICKY
            }
            ACTION_CONTROLLED -> {
                controlled = true
                show(controlled = true, projection != null)
            }
            ACTION_IDLE -> {
                controlled = false
                show(controlled = false, projection != null)
            }
            ACTION_CAPTURE -> {
                val code = intent.getIntExtra(EXTRA_CODE, 0)
                val data = intent.dataExtra()
                if (data != null) startCapture(code, data)
            }
            else -> show(controlled, projection != null)
        }
        return START_STICKY
    }

    override fun onDestroy() {
        releaseCapture()
        super.onDestroy()
    }

    private fun startCapture(code: Int, data: Intent) {
        show(controlled, capturing = true)
        releaseCapture()
        val manager = getSystemService(MediaProjectionManager::class.java)
        val token = try {
            manager.getMediaProjection(code, data)
        } catch (e: Exception) {
            Log.e(TAG, "MediaProjection was refused", e)
            null
        }
        if (token == null) {
            show(controlled, capturing = false)
            return
        }
        token.registerCallback(object : MediaProjection.Callback() {
            override fun onStop() {
                releaseCapture()
                show(controlled = false, capturing = false)
            }
        }, null)
        projection = token
        val metrics = DisplayMetrics()
        val wm = getSystemService(WindowManager::class.java)
        @Suppress("DEPRECATION")
        wm.defaultDisplay.getRealMetrics(metrics)
        val (w, h) = ScreenEncoder.fit(metrics.widthPixels, metrics.heightPixels, 1920)
        (application as PjApp).engine.captureSize(w, h)
        val engine = (application as PjApp).engine
        val next = ScreenEncoder(token, w, h, metrics.densityDpi) { bytes, width, height, key, us ->
            engine.submitVideo(bytes, width, height, key, us)
        }
        engine.onQuality = { _, bitrate, maxWidth ->
            val fitted = ScreenEncoder.fit(metrics.widthPixels, metrics.heightPixels, maxWidth)
            if (fitted.first != w) {
                // A smaller cap needs a new virtual display. Ask for a keyframe
                // at the current size until the user reconnects the capture.
                next.requestKeyframe()
            }
            next.applyQuality(30, bitrate)
        }
        engine.onKeyframe = { next.requestKeyframe() }
        encoder = next
        next.start()
        capturing = true
    }

    private fun show(controlled: Boolean, capturing: Boolean) {
        val channel = if (controlled) CHANNEL_LIVE else CHANNEL_ONLINE
        ensureChannels()
        val notification = notification(controlled)
        if (Build.VERSION.SDK_INT >= 29) {
            val type = when {
                capturing -> ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION
                Build.VERSION.SDK_INT >= 34 -> ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
                else -> 0
            }
            startForeground(NOTIF_ID, notification, type)
        } else {
            startForeground(NOTIF_ID, notification)
        }
        this.controlled = controlled
    }

    private fun notification(controlled: Boolean): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, HostService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val title = if (controlled) getString(R.string.being_controlled) else getString(R.string.host_online)
        val text = if (controlled) getString(R.string.being_controlled_body) else getString(R.string.host_online_body)
        return NotificationCompat.Builder(this, if (controlled) CHANNEL_LIVE else CHANNEL_ONLINE)
            .setSmallIcon(R.drawable.ic_stat)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(open)
            .setOngoing(true)
            .setCategory(Notification.CATEGORY_STATUS)
            .addAction(0, getString(R.string.stop), stop)
            .build()
    }

    private fun ensureChannels() {
        val manager = getSystemService(NotificationManager::class.java)
        val online = NotificationChannel(CHANNEL_ONLINE, getString(R.string.channel_online), NotificationManager.IMPORTANCE_LOW)
        val live = NotificationChannel(CHANNEL_LIVE, getString(R.string.channel_live), NotificationManager.IMPORTANCE_HIGH)
        manager.createNotificationChannel(online)
        manager.createNotificationChannel(live)
    }

    private fun releaseCapture() {
        runCatching { encoder?.stop() }
        encoder = null
        runCatching { projection?.stop() }
        projection = null
        capturing = false
    }

    private fun Intent.dataExtra(): Intent? {
        return if (Build.VERSION.SDK_INT >= 33) {
            getParcelableExtra(EXTRA_DATA, Intent::class.java)
        } else {
            @Suppress("DEPRECATION")
            getParcelableExtra(EXTRA_DATA)
        }
    }

    companion object {
        const val ACTION_CAPTURE = "us.spader.pjdesk.CAPTURE"
        const val ACTION_STOP = "us.spader.pjdesk.STOP"
        const val ACTION_CONTROLLED = "us.spader.pjdesk.CONTROLLED"
        const val ACTION_IDLE = "us.spader.pjdesk.IDLE"
        private const val EXTRA_CODE = "code"
        private const val EXTRA_DATA = "data"
        private const val NOTIF_ID = 7
        private const val CHANNEL_ONLINE = "pjdesk-online"
        private const val CHANNEL_LIVE = "pjdesk-live"
        private const val TAG = "pjdesk"

        @Volatile
        var capturing: Boolean = false

        fun arm(context: Context) {
            val intent = Intent(context, HostService::class.java)
            androidx.core.content.ContextCompat.startForegroundService(context, intent)
        }

        fun capture(context: Context, code: Int, data: Intent) {
            val intent = Intent(context, HostService::class.java).apply {
                action = ACTION_CAPTURE
                putExtra(EXTRA_CODE, code)
                putExtra(EXTRA_DATA, data)
            }
            androidx.core.content.ContextCompat.startForegroundService(context, intent)
        }

        fun controlled(context: Context, on: Boolean) {
            val intent = Intent(context, HostService::class.java).apply {
                action = if (on) ACTION_CONTROLLED else ACTION_IDLE
            }
            androidx.core.content.ContextCompat.startForegroundService(context, intent)
        }

        fun stop(context: Context) {
            context.startService(Intent(context, HostService::class.java).setAction(ACTION_STOP))
        }
    }
}
