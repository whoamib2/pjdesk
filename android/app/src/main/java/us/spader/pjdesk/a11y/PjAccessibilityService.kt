package us.spader.pjdesk.a11y

import android.accessibilityservice.AccessibilityService
import android.content.ComponentName
import android.content.Context
import android.media.AudioManager
import android.provider.Settings
import android.view.accessibility.AccessibilityEvent
import org.json.JSONObject
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
    }

    override fun onDestroy() {
        if (RemoteControl.service === this) RemoteControl.service = null
        super.onDestroy()
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {}

    override fun onInterrupt() {}

    fun gesture(event: JSONObject) {
        val (w, h) = screenSize(this)
        val description = gestureDescription(event, w, h) ?: return
        dispatchGesture(description, null, null)
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
    }
}
