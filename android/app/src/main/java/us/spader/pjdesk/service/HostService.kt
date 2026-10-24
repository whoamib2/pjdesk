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
import android.os.PowerManager
import android.util.DisplayMetrics
import android.view.Display
import android.view.WindowManager
import androidx.core.app.NotificationCompat
import us.spader.pjdesk.AppLog
import us.spader.pjdesk.MainActivity
import us.spader.pjdesk.PjApp
import us.spader.pjdesk.R
import us.spader.pjdesk.a11y.PjAccessibilityService
import us.spader.pjdesk.a11y.ProjectionConsent
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
    private var controlledBy = ""
    private var automatic = false
    /** Bumped before a capture is replaced so the old token's onStop cannot tear down the new one. */
    private var generation = 0
    private var reported = false
    private val main = Handler(Looper.getMainLooper())
    private var wake: PowerManager.WakeLock? = null
    /** Screen-share consent is still open. The CPU stays awake until it ends. */
    private var waitingConsent = false
    private var consentDrop: Runnable? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                AppLog.i("host service stop")
                (application as PjApp).engine.stop("The phone stopped sharing")
                clearConsentWait()
                releaseCapture()
                dropWake("stop")
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
                return START_NOT_STICKY
            }
            ACTION_CONTROLLED -> {
                controlled = true
                controlledBy = intent.getStringExtra(EXTRA_WHO).orEmpty()
                automatic = intent.getBooleanExtra(EXTRA_AUTOMATIC, false)
                holdWake("session")
                show(controlled = true, projection != null)
            }
            ACTION_IDLE -> {
                controlled = false
                controlledBy = ""
                automatic = false
                if (projection == null && !waitingConsent) dropWake("idle")
                show(controlled = false, projection != null)
            }
            ACTION_CONSENT -> {
                armConsentWake()
                show(controlled, projection != null)
            }
            ACTION_RELEASE -> {
                controlled = false
                controlledBy = ""
                clearConsentWait()
                AppLog.i("viewer left; stopping the projection")
                releaseCapture()
                dropWake("release")
                show(controlled = false, capturing = false)
            }
            ACTION_CAPTURE -> {
                val code = intent.getIntExtra(EXTRA_CODE, 0)
                val data = intent.dataExtra()
                if (data != null) startCapture(code, data) else fail("Screen capture permission denied")
            }
            ACTION_DISCARD -> {
                val code = intent.getIntExtra(EXTRA_CODE, 0)
                val data = intent.dataExtra()
                AppLog.i("consent arrived after the session ended; stopping the projection")
                clearConsentWait()
                if (data != null) stopGrantedToken(code, data)
                releaseCapture()
                if (!controlled) dropWake("consent discarded")
                show(controlled, capturing = false)
            }
            else -> show(controlled, projection != null)
        }
        return START_STICKY
    }

    override fun onDestroy() {
        clearConsentWait()
        releaseCapture()
        dropWake("destroy")
        super.onDestroy()
    }

    /**
     * Swiping the app away ends a session this phone is controlling.
     * The host service itself keeps running so this phone stays registered.
     */
    override fun onTaskRemoved(rootIntent: Intent?) {
        AppLog.i("task removed, ending the controller session")
        (application as PjApp).engine.stopView("closed")
        SessionNotice.hide(this)
        super.onTaskRemoved(rootIntent)
    }

    /**
     * Android 14+ allows getMediaProjection only while this service is already
     * in the foreground with type mediaProjection, and each consent token can
     * create one virtual display. Stopping the previous token must not flip
     * the service back to special-use before the new token is taken.
     */
    private fun startCapture(code: Int, data: Intent) {
        if (!viewerPresent()) {
            AppLog.i("consent arrived after the session ended; stopping the projection")
            clearConsentWait()
            stopGrantedToken(code, data)
            releaseCapture()
            if (!controlled) dropWake("consent discarded")
            show(controlled, capturing = false)
            return
        }
        clearConsentWait()
        holdWake("capture")
        val mine = ++generation
        reported = false
        val previousEncoder = encoder
        val previous = projection
        encoder = null
        projection = null
        syncCapture(false)
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
        if (!viewerPresent()) {
            AppLog.i("consent arrived after the session ended; stopping the projection")
            generation++
            runCatching { token.stop() }
            clearConsentWait()
            return
        }
        var rejected = false
        var fullConfirmed = false
        var readyToReport = false
        val (displayW, displayH) = displayPixels()
        fun rejectPartial(why: String) {
            if (rejected || generation != mine) return
            rejected = true
            val announced = reported
            generation++
            AppLog.i("single-app capture stopped ($why); asking for the entire screen")
            val restart = onConsentRestart
            onConsentRestart = null
            onCapture = null
            restart?.invoke()
            val tokenToStop = projection
            forgetProjection()
            runCatching { tokenToStop?.stop() }
            if (announced) (application as PjApp).engine.screenWait()
            val auto = preferAutoClick
            if (partialRetries >= PARTIAL_LIMIT) {
                partialRetries = 0
                AppLog.i("single-app capture persisted; showing the screen share prompt")
                main.postDelayed({ CapturePrompt.show(this) }, REASK_DELAY_MS)
                return
            }
            partialRetries += 1
            main.postDelayed({
                if (auto && PjAccessibilityService.ready(this)) {
                    ProjectionConsent.request(this)
                } else {
                    CapturePrompt.systemDialog(this)
                }
            }, REASK_DELAY_MS)
        }
        token.registerCallback(object : MediaProjection.Callback() {
            override fun onStop() {
                if (generation != mine) return
                generation++
                AppLog.i("MediaProjection stopped")
                forgetProjection()
                if (reported) {
                    val engine = (application as PjApp).engine
                    engine.showStatus("The phone stopped sharing")
                    engine.stop("The phone stopped sharing")
                } else {
                    fail("Screen capture stopped")
                }
            }

            override fun onCapturedContentResize(width: Int, height: Int) {
                if (generation != mine || rejected) return
                val (dw, dh) = displayPixels()
                if (EntireDisplay.isSingleApp(width, height, dw, dh)) {
                    AppLog.i("capture region ${width}x$height is not the display ${dw}x$dh")
                    rejectPartial("${width}x$height against ${dw}x$dh")
                } else {
                    fullConfirmed = true
                    partialRetries = 0
                    AppLog.i("capture region ${width}x$height covers the display ${dw}x$dh")
                    if (readyToReport) report(true, "")
                }
            }

            override fun onCapturedContentVisibilityChanged(isVisible: Boolean) {
                if (generation != mine || rejected || isVisible) return
                // Entire-display capture reports hidden only when the display is off.
                // A single app reports hidden while the display stays on.
                if (displayIsOn()) {
                    AppLog.i("captured region is hidden while the display is on")
                    rejectPartial("captured region hidden")
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
            onBeforeDisplay = { promoteToProjection() },
            onReady = {
                main.post {
                    if (generation != mine || rejected) return@post
                    if (!viewerPresent()) {
                        AppLog.i("viewer left before the picture was up; stopping the projection")
                        releaseCapture()
                        return@post
                    }
                    readyToReport = true
                    AppLog.i("screen capture started ${w}x$h display ${displayW}x$displayH")
                    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                        partialRetries = 0
                        report(true, "")
                    } else if (fullConfirmed) {
                        report(true, "")
                    } else {
                        main.postDelayed({
                            if (generation == mine && !rejected && !fullConfirmed) {
                                AppLog.i("capture region size was not reported; keeping this display capture")
                                partialRetries = 0
                                report(true, "")
                            }
                        }, REGION_WAIT_MS)
                    }
                }
            },
            onFailed = { error ->
                main.post {
                    if (generation != mine || rejected) return@post
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

    @Suppress("DEPRECATION")
    private fun displayPixels(): Pair<Int, Int> {
        val metrics = DisplayMetrics()
        getSystemService(WindowManager::class.java).defaultDisplay.getRealMetrics(metrics)
        return metrics.widthPixels to metrics.heightPixels
    }

    @Suppress("DEPRECATION")
    private fun displayIsOn(): Boolean {
        return getSystemService(WindowManager::class.java).defaultDisplay.state == Display.STATE_ON
    }

    private fun promoteToProjection(): Boolean {
        return try {
            val notification = notification(controlled)
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(NOTIF_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
            } else {
                startForeground(NOTIF_ID, notification)
            }
            AppLog.i("host foreground service mediaProjection")
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
        if (!controlled) dropWake("capture failed")
    }

    private fun report(ok: Boolean, reason: String) {
        if (reported) return
        reported = true
        onConsentRestart = null
        val engine = (application as PjApp).engine
        if (ok) syncCapture(true) else engine.captureDenied(reason.ifBlank { "Screen capture permission denied" })
        val callback = onCapture
        if (callback == null) {
            AppLog.i("capture result with no listener: ok=$ok $reason")
            return
        }
        callback(ok, reason)
    }

    private fun syncCapture(on: Boolean) {
        capturing = on
        runCatching { (application as PjApp).engine.setCapturing(on) }
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
        ControlNotice.sync(this)
    }

    private fun notification(controlled: Boolean): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, HostService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val title = if (controlled) {
            if (controlledBy.isBlank()) getString(R.string.being_controlled) else getString(R.string.being_controlled_by, controlledBy)
        } else {
            getString(R.string.host_online)
        }
        val text = when {
            controlled && automatic -> getString(R.string.accepted_automatically)
            controlled -> getString(R.string.being_controlled_body)
            else -> getString(R.string.host_online_body)
        }
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

    /** A consent grant arrived after the viewer left. Take the token and stop it. */
    private fun stopGrantedToken(code: Int, data: Intent) {
        if (promoteToProjection()) {
            val manager = getSystemService(MediaProjectionManager::class.java)
            val token = runCatching { manager.getMediaProjection(code, data) }.getOrNull()
            runCatching { token?.stop() }
        }
    }

    private fun viewerPresent(): Boolean = (application as PjApp).engine.hostAttached()

    private fun releaseCapture() {
        partialRetries = 0
        acceptOnSuccess = false
        generation++
        val token = projection
        forgetProjection()
        runCatching { token?.stop() }
        if (!controlled) dropWake("capture ended")
    }

    /**
     * The QUIC idle timeout is 15 seconds. A sleeping phone stops sending
     * keepalives and the PC closes the session while the share dialog is still
     * waiting. Hold the CPU for the minute that dialog is allowed to take.
     */
    private fun armConsentWake() {
        waitingConsent = true
        holdWake("waiting for screen share consent")
        consentDrop?.let { main.removeCallbacks(it) }
        val drop = Runnable {
            consentDrop = null
            if (!waitingConsent || projection != null || controlled) return@Runnable
            waitingConsent = false
            dropWake("consent wait ended")
        }
        consentDrop = drop
        main.postDelayed(drop, CONSENT_WAKE_MS)
        AppLog.i("host staying awake while waiting for screen share consent")
    }

    private fun clearConsentWait() {
        waitingConsent = false
        consentDrop?.let { main.removeCallbacks(it) }
        consentDrop = null
    }

    /** Keeps the CPU awake while a session is sharing. The screen can turn off. */
    private fun holdWake(why: String) {
        if (wake?.isHeld == true) return
        val pm = getSystemService(PowerManager::class.java) ?: return
        val lock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "pjdesk:host")
        lock.setReferenceCounted(false)
        lock.acquire(6 * 60 * 60 * 1000L)
        wake = lock
        AppLog.i("host wake lock held ($why)")
    }

    private fun dropWake(why: String) {
        val lock = wake ?: return
        wake = null
        val held = runCatching { lock.isHeld }.getOrDefault(false)
        if (held) runCatching { lock.release() }
        AppLog.i("host wake lock released ($why)")
    }

    /** Drops the encoder without stopping the token. onStop is already running in that case. */
    private fun forgetProjection() {
        val enc = encoder
        encoder = null
        projection = null
        syncCapture(false)
        runCatching { enc?.stop() }
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
        const val ACTION_DISCARD = "us.spader.pjdesk.DISCARD"
        const val ACTION_STOP = "us.spader.pjdesk.STOP"
        const val ACTION_CONTROLLED = "us.spader.pjdesk.CONTROLLED"
        const val ACTION_IDLE = "us.spader.pjdesk.IDLE"
        const val ACTION_RELEASE = "us.spader.pjdesk.RELEASE"
        const val ACTION_CONSENT = "us.spader.pjdesk.CONSENT"
        private const val EXTRA_CODE = "code"
        private const val EXTRA_DATA = "data"
        private const val EXTRA_WHO = "who"
        private const val EXTRA_AUTOMATIC = "automatic"
        private const val NOTIF_ID = 7
        private const val CHANNEL_ONLINE = "pjdesk-online"
        private const val CHANNEL_LIVE = "pjdesk-live"
        @Volatile
        var capturing: Boolean = false

        /** A system screen-share dialog is already on screen. Don't stack another one. */
        @Volatile
        var consentInProgress: Boolean = false

        /** Called on the main thread after a capture attempt. One result per consent. */
        @Volatile
        var onCapture: ((Boolean, String) -> Unit)? = null

        /**
         * Finish the consent activity without accepting or denying, because this
         * capture was a single app and a new entire-screen consent is opening.
         */
        @Volatile
        var onConsentRestart: (() -> Unit)? = null

        /** The consent that is opening was started by unattended access. */
        @Volatile
        var preferAutoClick: Boolean = false

        /**
         * A view or an in-app Allow is waiting on this capture. A single-app
         * result closes that activity and opens a new one, so the next success
         * still has to accept the connection.
         */
        @Volatile
        var acceptOnSuccess: Boolean = false

        /** How many single-app captures this session has already thrown away. */
        @Volatile
        var partialRetries: Int = 0

        private const val PARTIAL_LIMIT = 2
        private const val REGION_WAIT_MS = 1500L
        private const val REASK_DELAY_MS = 400L
        private const val CONSENT_WAKE_MS = 70_000L

        fun arm(context: Context) {
            val intent = Intent(context, HostService::class.java)
            androidx.core.content.ContextCompat.startForegroundService(context, intent)
        }

        /** Foreground service plus a partial wake lock for the consent minute. */
        fun consentWait(context: Context) {
            val intent = Intent(context, HostService::class.java).setAction(ACTION_CONSENT)
            androidx.core.content.ContextCompat.startForegroundService(context, intent)
        }

        fun capture(context: Context, code: Int, data: Intent) {
            if (!(context.applicationContext as PjApp).engine.hostAttached()) {
                AppLog.i("consent arrived after the session ended; stopping the projection")
                discard(context, code, data)
                return
            }
            AppLog.i("starting screen capture")
            val intent = Intent(context, HostService::class.java).apply {
                action = ACTION_CAPTURE
                putExtra(EXTRA_CODE, code)
                putExtra(EXTRA_DATA, data)
            }
            androidx.core.content.ContextCompat.startForegroundService(context, intent)
        }

        fun discard(context: Context, code: Int, data: Intent) {
            val intent = Intent(context, HostService::class.java).apply {
                action = ACTION_DISCARD
                putExtra(EXTRA_CODE, code)
                putExtra(EXTRA_DATA, data)
            }
            androidx.core.content.ContextCompat.startForegroundService(context, intent)
        }

        fun release(context: Context) {
            context.startService(Intent(context, HostService::class.java).setAction(ACTION_RELEASE))
        }

        fun controlled(context: Context, on: Boolean, who: String = "", automatic: Boolean = false) {
            val intent = Intent(context, HostService::class.java).apply {
                action = if (on) ACTION_CONTROLLED else ACTION_IDLE
                if (on) {
                    putExtra(EXTRA_WHO, who)
                    putExtra(EXTRA_AUTOMATIC, automatic)
                }
            }
            androidx.core.content.ContextCompat.startForegroundService(context, intent)
        }

        fun stop(context: Context) {
            context.startService(Intent(context, HostService::class.java).setAction(ACTION_STOP))
        }
    }
}
