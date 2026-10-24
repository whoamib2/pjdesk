package us.spader.pjdesk.service

import android.app.ActivityOptions
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import us.spader.pjdesk.AppLog

/**
 * Gets the share prompt onto the screen. A start from the application context
 * is what logcat records as "Background activity launch blocked". An
 * accessibility service is exempt, and a full-screen notification is the
 * other way when that service is off.
 */
object ConsentLaunch {
    /** Android 14+ withholds full-screen intents until the person allows them. */
    fun fullScreenAllowed(sdk: Int, canUse: Boolean): Boolean = sdk < 34 || canUse

    fun fullScreenAllowed(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < 34) return true
        val manager = context.getSystemService(NotificationManager::class.java) ?: return false
        return fullScreenAllowed(Build.VERSION.SDK_INT, manager.canUseFullScreenIntent())
    }

    fun pending(context: Context, request: Int, intent: Intent): PendingIntent {
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        if (Build.VERSION.SDK_INT < 31) {
            return PendingIntent.getActivity(context, request, intent, flags)
        }
        val options = ActivityOptions.makeBasic()
        if (Build.VERSION.SDK_INT >= 34) {
            options.setPendingIntentCreatorBackgroundActivityStartMode(
                ActivityOptions.MODE_BACKGROUND_ACTIVITY_START_ALLOWED,
            )
        } else {
            @Suppress("DEPRECATION")
            options.setPendingIntentBackgroundActivityLaunchAllowed(true)
        }
        return PendingIntent.getActivity(context, request, intent, flags, options.toBundle())
    }

    /**
     * Opens [intent] from the accessibility service when it is connected.
     * Returns false when that service is not bound. The caller leaves the
     * full-screen notification up instead of calling startActivity itself.
     */
    fun open(intent: Intent): Boolean {
        val service = RemoteControl.service ?: run {
            AppLog.i("screen share prompt stays on the notification; the accessibility service is not bound")
            return false
        }
        val ready = Intent(intent).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val opened = runCatching { service.startActivity(ready) }
        if (opened.isSuccess) {
            AppLog.i("screen share prompt opened from the accessibility service")
            return true
        }
        AppLog.e("accessibility service could not open the screen share prompt", opened.exceptionOrNull()!!)
        return false
    }

    fun fullScreenSettings(context: Context): Intent {
        return Intent(MANAGE_FULL_SCREEN).apply {
            data = Uri.parse("package:${context.packageName}")
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
    }

    private const val MANAGE_FULL_SCREEN = "android.settings.MANAGE_APP_USE_FULL_SCREEN_INTENT"
}
