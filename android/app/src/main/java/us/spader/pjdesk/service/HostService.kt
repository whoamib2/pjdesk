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
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.DisplayMetrics
import android.view.WindowManager
import androidx.core.app.NotificationCompat
import us.spader.pjdesk.AppLog
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
    /** Bumped before a capture is replaced so the old token's onStop cannot tear down the new one. */
    private var generation = 0
    private var reported = false
    private val main = Handler(Looper.getMainLooper())

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                AppLog.i("host service stop")
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
            ACTION_RELEASE -> releaseCapture()
            ACTION_CAPTURE -> {
                val code = intent.getIntExtra(EXTRA_CODE, 0)
                val data = intent.dataExtra()
                if (data != null) startCapture(code, data) else fail("Screen capture permission denied")
            }
            else -> show(controlled, projection != null)
        }
        return START_STICKY
    }

    override fun onDestroy() {
        releaseCapture()
        super.onDestroy()
    }

    /**
     * Android 14+ allows getMediaProjection only while this service is already
     * in the foreground with type mediaProjection, and each consent token can
     * create one virtual display. Stopping the previous token must not flip
     * the service back to special-use before the new token is taken.
     */
    private fun startCapture(code: Int, data: Intent) {
        val mine = ++generation
        reported = false
        val previousEncoder = encoder
        val previous = projection
        encoder = null
        projection = null
        capturing = false
        previousEncoder?.stop()
        runCatching { previous?.stop() }
        if (!promoteToProjection()) {
            fail("Could not start the screen-capture service")
            return
        }
        val manager = getSystemService(MediaProjectionManager::class.java)
        val token = try {
            manager.getMediaProjection(code, data)
        } catch (e: Exception) {
            AppLog.e("getMediaProjection failed", e)
            fail(captureMessage(e))
            return
        }
        if (token == null || generation != mine) {
            fail("Screen capture permission denied")
            return
        }
        token.registerCallback(object : MediaProjection.Callback() {
            override fun onStop() {
                if (generation != mine) return
                generation++
                AppLog.i("MediaProjection stopped")
                releaseCapture()
                if (reported) {
                    val engine = (application as PjApp).engine
                    engine.showError("Screen capture stopped")
                    engine.stop("Screen capture stopped")
                } else {
                    fail("Screen capture stopped")
                }
            }
        }, main)
        projection = token
        val metrics = DisplayMetrics()
        val wm = getSystemService(WindowManager::class.java)
        @Suppress("DEPRECATION")
        wm.defaultDisplay.getRealMetrics(metrics)
        val (w, h) = ScreenEncoder.fit(metrics.widthPixels, metrics.heightPixels, 1920)
        val engine = (application as PjApp).engine
        engine.captureSize(w, h)
        val next = ScreenEncoder(token, w, h, metrics.densityDpi) { bytes, width, height, key, us ->
            engine.submitVideo(bytes, width, height, key, us)
        }
        engine.onQuality = { _, bitrate, _ -> next.applyQuality(30, bitrate) }
        engine.onKeyframe = { next.requestKeyframe() }
        encoder = next
        next.start(
            onReady = {
                main.post {
                    if (generation != mine) return@post
                    capturing = true
                    AppLog.i("screen capture started ${w}x$h")
                    report(true, "")
                }
            },
            onFailed = { error ->
                main.post {
                    if (generation != mine) return@post
                    val message = error.ifBlank { "Screen capture failed" }
                    AppLog.e("encoder: $message")
                    val live = reported
                    releaseCapture()
                    if (live) {
                        val engine = (application as PjApp).engine
                        engine.showError(message)
                        engine.stop(message)
                    } else {
                        fail(message)
                    }
                }
            },
        )
    }

    private fun promoteToProjection(): Boolean {
        return try {
            val notification = notification(controlled)
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(NOTIF_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
            } else {
                startForeground(NOTIF_ID, notification)
            }
            true
        } catch (e: Exception) {
            AppLog.e("foreground service type mediaProjection was refused", e)
            false
        }
    }

    private fun fail(reason: String) {
        AppLog.e(reason)
        show(controlled, capturing = false)
        report(false, reason)
    }

    private fun report(ok: Boolean, reason: String) {
        if (reported) return
        reported = true
        val callback = onCapture
        if (callback == null) {
            AppLog.i("capture result with no listener: ok=$ok $reason")
            return
        }
        callback(ok, reason)
    }

    private fun captureMessage(error: Exception): String {
        val text = error.message.orEmpty()
        return if (text.contains("SecurityException", ignoreCase = true) || error is SecurityException) {
            "Screen capture permission denied"
        } else {
            text.ifBlank { "Screen capture permission denied" }
        }
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
        val enc = encoder
        val token = projection
        encoder = null
        projection = null
        capturing = false
        runCatching { enc?.stop() }
        runCatching { token?.stop() }
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
        const val ACTION_RELEASE = "us.spader.pjdesk.RELEASE"
        private const val EXTRA_CODE = "code"
        private const val EXTRA_DATA = "data"
        private const val NOTIF_ID = 7
        private const val CHANNEL_ONLINE = "pjdesk-online"
        private const val CHANNEL_LIVE = "pjdesk-live"
        @Volatile
        var capturing: Boolean = false

        /** Called on the main thread after a capture attempt. One result per consent. */
        @Volatile
        var onCapture: ((Boolean, String) -> Unit)? = null

        fun arm(context: Context) {
            val intent = Intent(context, HostService::class.java)
            androidx.core.content.ContextCompat.startForegroundService(context, intent)
        }

        fun capture(context: Context, code: Int, data: Intent) {
            AppLog.i("starting screen capture")
            val intent = Intent(context, HostService::class.java).apply {
                action = ACTION_CAPTURE
                putExtra(EXTRA_CODE, code)
                putExtra(EXTRA_DATA, data)
            }
            androidx.core.content.ContextCompat.startForegroundService(context, intent)
        }

        fun release(context: Context) {
            context.startService(Intent(context, HostService::class.java).setAction(ACTION_RELEASE))
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
