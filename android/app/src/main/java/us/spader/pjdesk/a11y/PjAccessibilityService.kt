package us.spader.pjdesk.a11y

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.AccessibilityServiceInfo
import android.accessibilityservice.GestureDescription
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.media.AudioManager
import android.provider.Settings
import android.view.accessibility.AccessibilityEvent
import org.json.JSONObject
import us.spader.pjdesk.AppLog
import us.spader.pjdesk.PjApp
import us.spader.pjdesk.service.RemoteControl
import us.spader.pjdesk.service.gestureDescription
import us.spader.pjdesk.service.screenSize
import us.spader.pjdesk.service.setFocusedText

/**
 * Remote taps, swipes, pinches, Back, Home, Recents, and text. The service
 * does nothing until a PJDesk session delivers an input event.
 */
class PjAccessibilityService : AccessibilityService() {
    override fun onServiceConnected() {
        RemoteControl.service = this
        dropAccessibilityButton()
        (application as? PjApp)?.engine?.setControl(true)
    }

    override fun onDestroy() {
        if (RemoteControl.service === this) RemoteControl.service = null
        (application as? PjApp)?.engine?.setControl(false)
        super.onDestroy()
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {}

    override fun onInterrupt() {}

    fun gesture(event: JSONObject) {
        val (w, h) = screenSize(this)
        val description = gestureDescription(event, w, h) ?: run {
            AppLog.i("gesture ignored")
            return
        }
        val sent = dispatchGesture(
            description,
            object : GestureResultCallback() {
                override fun onCancelled(gestureDescription: GestureDescription?) {
                    AppLog.i("gesture cancelled")
                }
            },
            null,
        )
        if (!sent) AppLog.i("gesture was not dispatched")
    }

    fun injectText(chars: String) {
        setFocusedText(rootInActiveWindow, chars)
    }

    fun special(which: Int) {
        when (which) {
            3 -> performGlobalAction(GLOBAL_ACTION_BACK)
            4 -> performGlobalAction(GLOBAL_ACTION_HOME)
            5 -> performGlobalAction(GLOBAL_ACTION_RECENTS)
            6 -> volume(AudioManager.ADJUST_RAISE)
            7 -> volume(AudioManager.ADJUST_LOWER)
            8 -> if (android.os.Build.VERSION.SDK_INT >= 28) {
                performGlobalAction(GLOBAL_ACTION_LOCK_SCREEN)
            } else {
                performGlobalAction(GLOBAL_ACTION_POWER_DIALOG)
            }
            9 -> performGlobalAction(GLOBAL_ACTION_NOTIFICATIONS)
        }
    }

    /**
     * The floating blue button is the system accessibility shortcut. PJDesk does
     * not request it. Clear the flag if a previous build or the system set it,
     * and drop this service from the shortcut target list when the platform allows.
     */
    private fun dropAccessibilityButton() {
        val info = serviceInfo ?: return
        val without = info.flags and AccessibilityServiceInfo.FLAG_REQUEST_ACCESSIBILITY_BUTTON.inv()
        if (without != info.flags) {
            info.flags = without
            serviceInfo = info
            AppLog.i("cleared the accessibility button flag")
        }
        val resolver = contentResolver
        for (key in listOf("accessibility_button_targets", "accessibility_shortcut_target_service")) {
            val current = Settings.Secure.getString(resolver, key) ?: continue
            if (!current.contains(packageName)) continue
            val next = current.split(':').filter { it.isNotBlank() && !it.contains(packageName) }.joinToString(":")
            try {
                val wrote = Settings.Secure.putString(resolver, key, next)
                AppLog.i(if (wrote) "removed this service from $key" else "could not change $key")
            } catch (e: SecurityException) {
                AppLog.i("this service is still listed in $key")
            }
        }
    }

    private fun volume(direction: Int) {
        val audio = getSystemService(AudioManager::class.java)
        audio.adjustStreamVolume(AudioManager.STREAM_MUSIC, direction, AudioManager.FLAG_SHOW_UI)
    }

    companion object {
        fun enabled(context: Context): Boolean {
            val expected = ComponentName(context, PjAccessibilityService::class.java)
            val raw = Settings.Secure.getString(context.contentResolver, Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES)
                ?: return false
            return raw.split(':').any { ComponentName.unflattenFromString(it) == expected }
        }

        /** Enabled in Settings and connected, so taps and typing can be delivered. */
        fun ready(context: Context): Boolean = enabled(context) && RemoteControl.service != null

        fun openSettings(context: Context) {
            val intent = Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            runCatching { context.startActivity(intent) }
        }
    }
}
