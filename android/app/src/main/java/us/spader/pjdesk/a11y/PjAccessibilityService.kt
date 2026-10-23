package us.spader.pjdesk.a11y

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.media.AudioManager
import android.os.Build
import android.provider.Settings
import android.graphics.Path
import android.graphics.Rect
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
    private val held = ArrayList<AccessibilityNodeInfo>()
    private val heldWindows = ArrayList<AccessibilityWindowInfo>()

    override fun onServiceConnected() {
        AppLog.i("accessibility service connected service=${System.identityHashCode(this)}")
        RemoteControl.service = this
        runCatching { us.spader.pjdesk.service.ControlNotice.sync(this) }
        runCatching { (application as? PjApp)?.engine?.setControl(true) }
            .onFailure { AppLog.e("could not report control on", it) }
    }

    override fun onUnbind(intent: Intent?): Boolean {
        AppLog.i("accessibility service unbound service=${System.identityHashCode(this)}")
        detach()
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        AppLog.i("accessibility service destroyed service=${System.identityHashCode(this)}")
        detach()
        super.onDestroy()
    }

    private fun detach() {
        releaseNodes()
        if (RemoteControl.service !== this) return
        RemoteControl.service = null
        runCatching { us.spader.pjdesk.service.ControlNotice.sync(this) }
        runCatching { (application as? PjApp)?.engine?.setControl(false) }
            .onFailure { AppLog.e("could not report control off", it) }
    }

    private fun releaseNodes() {
        held.asReversed().forEach { runCatching { it.recycle() } }
        held.clear()
        heldWindows.forEach { runCatching { it.recycle() } }
        heldWindows.clear()
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        // The consent watch polls on its own. Driving it from these events made
        // the deadline depend on how chatty the dialog was.
    }

    fun collectConsent(): List<ConsentNode> {
        return runCatching { snapshot() }.getOrElse {
            AppLog.e("could not read the screen share dialog", it)
            emptyList()
        }
    }

    fun activate(index: Int, node: ConsentNode?, why: String): ConsentWatch.ClickResult {
        val info = held.getOrNull(index)
        val action = info?.let {
            runCatching { it.performAction(AccessibilityNodeInfo.ACTION_CLICK) }.getOrDefault(false)
        } ?: false
        if (action) return ConsentWatch.ClickResult(action = true, gesture = false, bounds = boundsText(node))
        val gestured = tap(node, why)
        return ConsentWatch.ClickResult(action = false, gesture = gestured, bounds = boundsText(node))
    }

    private fun boundsText(node: ConsentNode?): String {
        if (node == null) return "none"
        return "${node.left},${node.top},${node.right},${node.bottom}"
    }

    /** A system dialog often ignores ACTION_CLICK. A short tap at its center does land. */
    private fun tap(node: ConsentNode?, why: String): Boolean {
        if (node == null) return false
        val width = node.right - node.left
        val height = node.bottom - node.top
        if (width <= 0 || height <= 0) return false
        val path = Path().apply { moveTo(node.left + width / 2f, node.top + height / 2f) }
        val stroke = GestureDescription.StrokeDescription(path, 0, 60)
        val gesture = GestureDescription.Builder().addStroke(stroke).build()
        return runCatching {
            dispatchGesture(
                gesture,
                object : GestureResultCallback() {
                    override fun onCompleted(gestureDescription: GestureDescription?) {
                        main.post { ConsentWatcher.gestureFinished(true, why) }
                    }

                    override fun onCancelled(gestureDescription: GestureDescription?) {
                        main.post { ConsentWatcher.gestureFinished(false, why) }
                    }
                },
                null,
            )
        }.getOrDefault(false)
    }

    private fun snapshot(): List<ConsentNode> {
        releaseNodes()
        val nodes = ArrayList<ConsentNode>()
        val open = this.windows ?: return nodes
        for (window in open) {
            heldWindows.add(window)
            val root = window.root
            if (root != null) append(root, windowClass(window), nodes)
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
        val bounds = Rect()
        runCatching { info.getBoundsInScreen(bounds) }
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
                left = bounds.left,
                top = bounds.top,
                right = bounds.right,
                bottom = bounds.bottom,
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

        /**
         * The PJDesk page in Accessibility settings. Android 13 and later open
         * that service directly. Older releases open the Accessibility list.
         * This never writes secure settings. An update turns the service off,
         * and only the person at the phone can turn it back on.
         */
        fun detailsIntent(context: Context): Intent {
            val component = ComponentName(context, PjAccessibilityService::class.java)
            val action = accessibilitySettingsAction(Build.VERSION.SDK_INT)
            return Intent(action).apply {
                if (action == DETAILS_ACTION) putExtra(Intent.EXTRA_COMPONENT_NAME, component)
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
        }

        fun openSettings(context: Context) {
            val primary = detailsIntent(context)
            val opened = runCatching { context.startActivity(primary) }
            if (opened.isSuccess) return
            AppLog.e("could not open the PJDesk accessibility page", opened.exceptionOrNull()!!)
            if (primary.action == Settings.ACTION_ACCESSIBILITY_SETTINGS) return
            runCatching {
                context.startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }
        }

        fun accessibilitySettingsAction(sdk: Int): String =
            if (sdk >= 33) DETAILS_ACTION else Settings.ACTION_ACCESSIBILITY_SETTINGS

        private const val DETAILS_ACTION = "android.settings.ACCESSIBILITY_DETAILS_SETTINGS"
    }
}
