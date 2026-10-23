package us.spader.pjdesk.service

import android.app.Activity
import android.app.AlertDialog
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.app.NotificationCompat
import us.spader.pjdesk.PjApp
import us.spader.pjdesk.R

/**
 * The view-request prompt. A high-importance notification can show this over
 * the lock screen when the app is in the background. Android may still show
 * its own screen-share consent after Allow.
 */
class ViewRequestActivity : ComponentActivity() {
    private var who: String = "Someone"
    private var startCapture: Boolean = false

    private val projection = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val data = result.data
        val engine = (application as PjApp).engine
        if (result.resultCode == RESULT_OK && data != null) {
            HostService.onCapture = { ok, reason ->
                runOnUiThread {
                    HostService.onCapture = null
                    if (ok) engine.accept() else engine.decline(reason.ifBlank { "Screen capture permission denied" })
                    ViewPrompt.cancel(this)
                    finish()
                }
            }
            HostService.capture(this, result.resultCode, data)
        } else {
            engine.decline("Screen capture permission denied")
            ViewPrompt.cancel(this)
            finish()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= 27) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
        }
        who = intent.getStringExtra(EXTRA_WHO).orEmpty().ifBlank { "Someone" }
        val auto = intent.getBooleanExtra(EXTRA_AUTO, false)
        val allow = intent.getBooleanExtra(EXTRA_ALLOW, false)
        if (auto || allow) {
            startCapture = true
            return
        }
        val engine = (application as PjApp).engine
        AlertDialog.Builder(this)
            .setTitle("$who wants to view your screen")
            .setMessage("Allow starts the session. Android then asks to share the screen, unless this phone is already sharing. That system prompt cannot be skipped.")
            .setPositiveButton("Allow") { _, _ ->
                if (HostService.capturing) {
                    engine.accept()
                    ViewPrompt.cancel(this)
                    finish()
                } else {
                    launchProjection()
                }
            }
            .setNegativeButton("Deny") { _, _ ->
                engine.decline("Declined. $who was not allowed to view this phone.")
                ViewPrompt.cancel(this)
                finish()
            }
            .setOnCancelListener {
                engine.decline("Declined. $who was not allowed to view this phone.")
                ViewPrompt.cancel(this)
                finish()
            }
            .show()
    }

    override fun onStart() {
        super.onStart()
        if (!startCapture) return
        startCapture = false
        val engine = (application as PjApp).engine
        if (HostService.capturing) {
            engine.accept()
            ViewPrompt.cancel(this)
            finish()
        } else {
            launchProjection()
        }
    }

    private fun launchProjection() {
        val manager = getSystemService(MediaProjectionManager::class.java)
        projection.launch(manager.createScreenCaptureIntent())
    }

    companion object {
        const val EXTRA_WHO = "who"
        const val EXTRA_AUTO = "auto"
        const val EXTRA_ALLOW = "allow"
    }
}

class ViewRequestReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val who = intent.getStringExtra(ViewRequestActivity.EXTRA_WHO).orEmpty().ifBlank { "Someone" }
        (context.applicationContext as PjApp).engine.decline("Declined. $who was not allowed to view this phone.")
        ViewPrompt.cancel(context)
    }
}

object ViewPrompt {
    private const val CHANNEL = "pjdesk-view-request"
    private const val NOTIF = 8

    fun show(context: Context, who: String, bypass: Boolean) {
        val app = context.applicationContext
        val manager = app.getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            val channel = NotificationChannel(CHANNEL, "View requests", NotificationManager.IMPORTANCE_HIGH)
            channel.description = "Someone wants to view this phone"
            channel.lockscreenVisibility = Notification.VISIBILITY_PUBLIC
            manager.createNotificationChannel(channel)
        }
        val open = Intent(app, ViewRequestActivity::class.java).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            putExtra(ViewRequestActivity.EXTRA_WHO, who)
            putExtra(ViewRequestActivity.EXTRA_AUTO, bypass)
        }
        val allow = Intent(open).putExtra(ViewRequestActivity.EXTRA_ALLOW, true)
        val content = pending(app, 0, open)
        val allowIntent = pending(app, 2, allow)
        val deny = PendingIntent.getBroadcast(
            app,
            1,
            Intent(app, ViewRequestReceiver::class.java).putExtra(ViewRequestActivity.EXTRA_WHO, who),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val title = if (bypass) "Share your screen" else "$who wants to view your screen"
        val body = if (bypass) {
            "This request is already allowed. Android may still ask you to share the screen."
        } else {
            "Allow or deny this view request."
        }
        val builder = NotificationCompat.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat)
            .setContentTitle(title)
            .setContentText(body)
            .setPriority(NotificationCompat.PRIORITY_MAX)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setContentIntent(content)
            .setFullScreenIntent(content, true)
            .setAutoCancel(false)
            .setOngoing(true)
        if (!bypass) {
            builder.addAction(0, "Allow", allowIntent)
            builder.addAction(0, "Deny", deny)
        }
        manager.notify(NOTIF, builder.build())
        runCatching { app.startActivity(open) }
    }

    fun cancel(context: Context) {
        context.applicationContext.getSystemService(NotificationManager::class.java).cancel(NOTIF)
    }

    private fun pending(context: Context, request: Int, intent: Intent): PendingIntent {
        return PendingIntent.getActivity(
            context,
            request,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }
}

/**
 * Screen-share consent after a password was accepted. The heads-up notification
 * can appear while PJDesk is in the background. Tapping Allow starts Android's
 * own MediaProjection dialog. Appearing over the lock screen does not share
 * the screen by itself.
 */
class CaptureConsentActivity : ComponentActivity() {
    private var launchNow: Boolean = false

    private val projection = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        HostService.consentInProgress = false
        val data = result.data
        val engine = (application as PjApp).engine
        if (result.resultCode == RESULT_OK && data != null) {
            HostService.onCapture = { ok, reason ->
                runOnUiThread {
                    HostService.onCapture = null
                    if (!ok) engine.captureDenied(reason.ifBlank { "Screen capture permission denied" })
                    CapturePrompt.cancel(this)
                    finish()
                }
            }
            HostService.capture(this, result.resultCode, data)
        } else {
            engine.captureDenied("Screen capture permission denied")
            CapturePrompt.cancel(this)
            finish()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= 27) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
        }
        HostService.arm(this)
        val allow = intent.getBooleanExtra(EXTRA_ALLOW, false)
        if (HostService.capturing) {
            (application as PjApp).engine.setCapturing(true)
            CapturePrompt.cancel(this)
            finish()
            return
        }
        if (allow) {
            launchNow = true
            return
        }
        AlertDialog.Builder(this)
            .setTitle(getString(R.string.allow_screen_share))
            .setMessage(getString(R.string.allow_screen_share_body))
            .setPositiveButton(getString(R.string.allow_screen_share)) { _, _ -> launchProjection() }
            .setNegativeButton("Deny") { _, _ ->
                (application as PjApp).engine.captureDenied("Screen capture permission denied")
                CapturePrompt.cancel(this)
                finish()
            }
            .setOnCancelListener {
                (application as PjApp).engine.captureDenied("Screen capture permission denied")
                CapturePrompt.cancel(this)
                finish()
            }
            .show()
    }

    override fun onStart() {
        super.onStart()
        if (!launchNow) return
        launchNow = false
        if (HostService.capturing) {
            (application as PjApp).engine.setCapturing(true)
            CapturePrompt.cancel(this)
            finish()
        } else {
            launchProjection()
        }
    }

    private fun launchProjection() {
        HostService.consentInProgress = true
        val manager = getSystemService(MediaProjectionManager::class.java)
        projection.launch(manager.createScreenCaptureIntent())
    }

    companion object {
        const val EXTRA_ALLOW = "allow"
    }
}

object CapturePrompt {
    private const val CHANNEL = "pjdesk-screen-share"
    private const val NOTIF = 9

    fun show(context: Context, launch: Boolean = true) {
        val app = context.applicationContext
        if (HostService.capturing) {
            (app as PjApp).engine.setCapturing(true)
            return
        }
        if (launch && HostService.consentInProgress) return
        HostService.arm(app)
        val manager = app.getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            val channel = NotificationChannel(CHANNEL, app.getString(R.string.channel_share), NotificationManager.IMPORTANCE_HIGH)
            channel.description = app.getString(R.string.allow_screen_share_body)
            channel.lockscreenVisibility = Notification.VISIBILITY_PUBLIC
            manager.createNotificationChannel(channel)
        }
        val open = Intent(app, CaptureConsentActivity::class.java).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
        }
        val allow = Intent(open).putExtra(CaptureConsentActivity.EXTRA_ALLOW, true)
        val content = PendingIntent.getActivity(
            app,
            3,
            open,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val allowIntent = PendingIntent.getActivity(
            app,
            4,
            allow,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = NotificationCompat.Builder(app, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat)
            .setContentTitle(app.getString(R.string.allow_screen_share))
            .setContentText(app.getString(R.string.allow_screen_share_body))
            .setPriority(NotificationCompat.PRIORITY_MAX)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setContentIntent(content)
            .setFullScreenIntent(content, true)
            .setAutoCancel(false)
            .setOngoing(true)
            .addAction(0, app.getString(R.string.allow_screen_share), allowIntent)
            .build()
        manager.notify(NOTIF, notification)
        if (launch) runCatching { app.startActivity(open) }
    }

    /**
     * The system dialog is already up and the auto-click did not land.
     * Keep a visible prompt without opening a second capture dialog.
     */
    fun nudge(context: Context) {
        val app = context.applicationContext
        if (HostService.capturing) return
        show(app, launch = false)
    }

    fun cancel(context: Context) {
        context.applicationContext.getSystemService(NotificationManager::class.java).cancel(NOTIF)
    }
}
