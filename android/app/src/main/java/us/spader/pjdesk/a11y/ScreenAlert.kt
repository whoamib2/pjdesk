package us.spader.pjdesk.a11y

import android.app.Activity
import android.app.KeyguardManager
import android.content.Context
import android.os.Build
import android.os.PowerManager
import android.view.WindowManager
import us.spader.pjdesk.AppLog

/**
 * Turns the screen on and asks the keyguard to step aside so the system
 * share dialog can be tapped. A secure lock still needs the person to unlock.
 */
object ScreenAlert {
    fun locked(context: Context): Boolean {
        val km = context.getSystemService(KeyguardManager::class.java) ?: return false
        return km.isKeyguardLocked
    }

    /** Best-effort screen-on before the consent activity is in front. */
    fun wake(context: Context) {
        val pm = context.getSystemService(PowerManager::class.java) ?: return
        if (pm.isInteractive && !locked(context)) return
        AppLog.i("waking the screen for screen share interactive=${pm.isInteractive} keyguard=${locked(context)}")
        runCatching {
            @Suppress("DEPRECATION")
            val lock = pm.newWakeLock(
                PowerManager.SCREEN_BRIGHT_WAKE_LOCK or PowerManager.ACQUIRE_CAUSES_WAKEUP or PowerManager.ON_AFTER_RELEASE,
                "pjdesk:consent",
            )
            lock.setReferenceCounted(false)
            lock.acquire(15_000L)
        }.onFailure { AppLog.e("could not wake the screen", it) }
    }

    fun prepare(activity: Activity) {
        if (Build.VERSION.SDK_INT >= 27) {
            activity.setShowWhenLocked(true)
            activity.setTurnScreenOn(true)
        } else {
            @Suppress("DEPRECATION")
            activity.window.addFlags(
                WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED or WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON,
            )
        }
        activity.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        wake(activity)
        val km = activity.getSystemService(KeyguardManager::class.java) ?: return
        if (!km.isKeyguardLocked) return
        AppLog.i("keyguard is locked; asking to show the screen share dialog over the lock screen")
        runCatching {
            km.requestDismissKeyguard(
                activity,
                object : KeyguardManager.KeyguardDismissCallback() {
                    override fun onDismissSucceeded() {
                        AppLog.i("keyguard dismissed")
                    }

                    override fun onDismissCancelled() {
                        AppLog.i("keyguard dismiss cancelled; the screen share dialog cannot be tapped while locked")
                    }

                    override fun onDismissError() {
                        AppLog.i("keyguard dismiss failed; the screen share dialog cannot be tapped while locked")
                    }
                },
            )
        }.onFailure { AppLog.e("could not dismiss the keyguard", it) }
    }
}
