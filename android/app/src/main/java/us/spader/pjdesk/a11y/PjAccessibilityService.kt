package us.spader.pjdesk.a11y

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.AccessibilityServiceInfo
import android.accessibilityservice.GestureDescription
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.media.AudioManager
import android.provider.Settings
import android.os.Handler
import android.os.Looper
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
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
    private val main = Handler(Looper.getMainLooper())
    private var polls = 0
    private var onGiveUp: (() -> Unit)? = null
    private var lastDialog = ""
    private val held = ArrayList<AccessibilityNodeInfo>()

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

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (!ProjectionConsent.armed) return
        val type = event?.eventType ?: return
        if (type != AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED && type != AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED) return
        main.post { pollConsent() }
    }

    fun beginConsentWatch(giveUp: () -> Unit) {
        polls = 0
        lastDialog = ""
        onGiveUp = giveUp
        main.removeCallbacks(poll)
        main.post(poll)
    }

    private val poll = Runnable { pollConsent() }

    private fun pollConsent() {
        if (!ProjectionConsent.armed) return
        polls += 1
        val nodes = snapshot()
        val step = ProjectionConsent.decide(nodes)
        if (ProjectionConsent.looksLikeConsent(nodes)) {
            val mode = nodes.firstOrNull { ProjectionConsent.isModeControl(it) }
            val button = nodes.firstOrNull { it.viewId == "android:id/button1" }
            val line = "screen share dialog mode='${mode?.text.orEmpty()}' child='${mode?.subtree.orEmpty()}' button='${button?.text.orEmpty()}' step=$step"
            if (line != lastDialog) {
                lastDialog = line
                AppLog.i(line)
            }
        }
        when (step) {
            is ConsentStep.Click -> {
                val info = held.getOrNull(step.index)
                val fired = info?.performAction(AccessibilityNodeInfo.ACTION_CLICK) == true
                AppLog.i("screen share auto-click ${step.why} fired=$fired")
                if (fired && step.why == "share") {
                    ProjectionConsent.markClicked()
                    ProjectionConsent.stopWatching()
                }
            }
            ConsentStep.Wait -> Unit
        }
        if (ProjectionConsent.armed && polls < POLL_LIMIT) {
            main.postDelayed(poll, POLL_MS)
        } else if (ProjectionConsent.armed) {
            val giveUp = onGiveUp
            onGiveUp = null
            giveUp?.invoke()
        }
    }

    private fun snapshot(): List<ConsentNode> {
        held.asReversed().forEach { runCatching { it.recycle() } }
        held.clear()
        val nodes = ArrayList<ConsentNode>()
        val open = this.windows ?: return nodes
        for (window in open) {
            val root = window.root
            if (root != null) append(root, windowClass(window), nodes)
            runCatching { window.recycle() }
        }
        return nodes
    }

    private fun append(info: AccessibilityNodeInfo, windowClass: String, nodes: ArrayList<ConsentNode>) {
        val text = info.text?.toString().orEmpty()
        val description = info.contentDescription?.toString().orEmpty()
        val own = listOf(text, description).filter { it.isNotBlank() }.joinToString(" ")
        val childText = StringBuilder()
        val children = ArrayList<AccessibilityNodeInfo>()
        for (i in 0 until info.childCount) {
            val child = info.getChild(i) ?: continue
            children.add(child)
            val bit = child.text?.toString().orEmpty()
            if (bit.isNotBlank()) {
                if (childText.isNotEmpty()) childText.append(' ')
                childText.append(bit)
            }
        }
        val subtree = listOf(own, childText.toString()).filter { it.isNotBlank() }.joinToString(" ")
        held.add(info)
        nodes.add(
            ConsentNode(
                packageName = info.packageName?.toString().orEmpty(),
                className = info.className?.toString().orEmpty(),
                windowClass = windowClass,
                viewId = info.viewIdResourceName.orEmpty(),
                text = own,
                subtree = subtree,
                clickable = info.isClickable,
                enabled = info.isEnabled,
                checked = info.isChecked,
            ),
        )
        children.forEach { append(it, windowClass, nodes) }
    }

    private fun windowClass(window: AccessibilityWindowInfo): String {
        return window.title?.toString().orEmpty()
    }

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
        val nav = navAction(which)
        if (nav != null) {
            fire(which, nav)
            return
        }
        when (which) {
            6 -> volume(AudioManager.ADJUST_RAISE)
            7 -> volume(AudioManager.ADJUST_LOWER)
            8 -> fire(
                which,
                if (android.os.Build.VERSION.SDK_INT >= 28) GLOBAL_ACTION_LOCK_SCREEN else GLOBAL_ACTION_POWER_DIALOG,
            )
            9 -> fire(which, GLOBAL_ACTION_NOTIFICATIONS)
        }
    }

    private fun fire(which: Int, action: Int) {
        val fired = performGlobalAction(action)
        AppLog.i("global action which=$which action=$action fired=$fired")
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

        /**
         * Back, Home, and Recents. These are the platform global actions, so they
         * run on gesture navigation as well as the three-button bar.
         * 3, 4, and 5 are the indexes the Windows viewer sends.
         */
        fun navAction(which: Int): Int? = when (which) {
            3 -> AccessibilityService.GLOBAL_ACTION_BACK
            4 -> AccessibilityService.GLOBAL_ACTION_HOME
            5 -> AccessibilityService.GLOBAL_ACTION_RECENTS
            else -> null
        }

        fun openSettings(context: Context) {
            val intent = Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            runCatching { context.startActivity(intent) }
        }

        fun watchConsent(giveUp: () -> Unit) {
            val service = RemoteControl.service
            if (service == null) {
                giveUp()
                return
            }
            service.beginConsentWatch(giveUp)
        }

        private const val POLL_LIMIT = 30
        private const val POLL_MS = 400L
    }
}
