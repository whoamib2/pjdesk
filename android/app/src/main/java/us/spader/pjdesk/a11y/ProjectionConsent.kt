package us.spader.pjdesk.a11y

import android.content.Context
import android.content.Intent
import us.spader.pjdesk.AppLog
import us.spader.pjdesk.service.CapturePrompt
import us.spader.pjdesk.service.HostService

/**
 * One node copied out of an accessibility window. [text] is this node's own
 * text. [subtree] includes descendant text, which is where a spinner shows the
 * selected share mode.
 */
data class ConsentNode(
    val packageName: String = "",
    val className: String = "",
    val windowClass: String = "",
    val viewId: String = "",
    val text: String = "",
    val subtree: String = "",
    val clickable: Boolean = false,
    val enabled: Boolean = true,
    val checked: Boolean = false,
)

sealed class ConsentStep {
    data class Click(val index: Int, val why: String) : ConsentStep()
    data object Wait : ConsentStep()
}

/**
 * Chooses the next tap on Android's MediaProjection dialog.
 * Resource ids come first. Text is the fallback, including the labels Pixel
 * uses ("Share entire screen", "Share screen", "Start now") and a few other
 * locales. A two-row list that has one "single app" row taps the other row.
 * Start is not tapped while the spinner still shows a single app. The
 * spinner's own text is the selected mode; descendant text can mention both.
 */
object ProjectionConsent {
    const val NOTICE = "Accepted automatically"
    const val SYSTEM_UI = "com.android.systemui"
    private const val MODE_OPTIONS = "screen_share_mode_options"
    private const val MODE_SPINNER = "screen_share_mode_spinner"

    @Volatile
    var armed: Boolean = false
        private set

    @Volatile
    private var automatic: Boolean = false

    fun request(context: Context) {
        val app = context.applicationContext
        if (!PjAccessibilityService.ready(app)) {
            AppLog.i("unattended access needs the accessibility service; showing the screen share prompt")
            CapturePrompt.show(app)
            return
        }
        HostService.arm(app)
        HostService.preferAutoClick = true
        armed = true
        AppLog.i("unattended access opening the screen share dialog")
        automatic = false
        PjAccessibilityService.watchConsent {
            armed = false
            if (HostService.capturing) return@watchConsent
            AppLog.i("auto-accept could not click the screen share dialog")
            if (HostService.consentInProgress) CapturePrompt.nudge(app) else CapturePrompt.show(app)
        }
        val open = Intent(app, us.spader.pjdesk.service.CaptureConsentActivity::class.java).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            putExtra(us.spader.pjdesk.service.CaptureConsentActivity.EXTRA_ALLOW, true)
        }
        runCatching { app.startActivity(open) }.onFailure {
            AppLog.i("could not open the screen share dialog")
            armed = false
            CapturePrompt.show(app)
        }
    }

    /** The screen was already being shared, so nobody had to tap the dialog. */
    fun markAlreadySharing() {
        automatic = true
    }

    fun markClicked() {
        automatic = true
    }

    /** The share button was clicked. Stop polling without forgetting that it fired. */
    fun stopWatching() {
        armed = false
    }

    fun takeAutomatic(): Boolean {
        val on = automatic
        automatic = false
        armed = false
        return on
    }

    fun reset() {
        armed = false
        automatic = false
    }

    fun decide(nodes: List<ConsentNode>): ConsentStep {
        if (!looksLikeConsent(nodes)) return ConsentStep.Wait
        val entire = nodes.indexOfFirst { isEntireChoice(it) }
        val mode = nodes.indexOfFirst { isModeControl(it) }
        val modeEntire = mode >= 0 && modeIsEntire(nodes[mode])
        if (mode >= 0 && !modeEntire) {
            if (entire >= 0) return ConsentStep.Click(entire, "entire screen")
            val other = otherShareRow(nodes)
            if (other >= 0) return ConsentStep.Click(other, "entire screen")
            if (nodes[mode].enabled) return ConsentStep.Click(mode, "open share mode")
            return ConsentStep.Wait
        } else if (mode < 0 && entire >= 0 && !nodes[entire].checked) {
            return ConsentStep.Click(entire, "entire screen")
        }
        val positive = nodes.indexOfFirst { isPositive(it) }
        if (positive >= 0 && (mode < 0 || modeEntire)) return ConsentStep.Click(positive, "share")
        return ConsentStep.Wait
    }

    /** The spinner's own text is the selection. Child text can name both choices. */
    fun modeIsEntire(node: ConsentNode): Boolean {
        if (isSingleApp(node.text)) return false
        val shown = node.text.ifBlank { node.subtree }
        if (isSingleApp(shown)) return false
        return isEntireLabel(shown)
    }

    private fun otherShareRow(nodes: List<ConsentNode>): Int {
        val rows = nodes.withIndex().filter { (index, node) ->
            node.viewId == "android:id/text1" && node.clickable && node.enabled && node.text.isNotBlank() && index != nodes.indexOfFirst { isModeControl(it) }
        }
        if (rows.size != 2) return -1
        val single = rows.filter { isSingleApp(it.value.text) }
        val other = rows.filter { !isSingleApp(it.value.text) }
        if (single.size == 1 && other.size == 1) return other[0].index
        return -1
    }

    fun looksLikeConsent(nodes: List<ConsentNode>): Boolean {
        if (nodes.any { isModeControl(it) || it.className.contains("MediaProjection") || it.windowClass.contains("MediaProjection") }) {
            return true
        }
        if (nodes.any { isEntireLabel(it.text) || isEntireLabel(it.subtree) || isSingleApp(it.text) }) return true
        return nodes.any { it.packageName == SYSTEM_UI && it.viewId == "android:id/button1" && isConfirm(it.text) }
    }

    fun isModeControl(node: ConsentNode): Boolean {
        val id = node.viewId.substringAfterLast('/')
        return id == MODE_OPTIONS || id == MODE_SPINNER
    }

    fun isEntireChoice(node: ConsentNode): Boolean {
        if (isModeControl(node) || !node.enabled) return false
        if (!(node.clickable || node.checked)) return false
        return isEntireLabel(node.text)
    }

    fun isPositive(node: ConsentNode): Boolean {
        if (!node.clickable || !node.enabled || isModeControl(node)) return false
        if (node.viewId == "android:id/button2") return false
        if (isSingleApp(node.text) || isEntireLabel(node.text)) return false
        val confirm = isConfirm(node.text)
        if (node.viewId == "android:id/button1") return confirm || node.text.isBlank()
        return confirm && (node.packageName == SYSTEM_UI || node.windowClass.contains("MediaProjection"))
    }

    fun isConfirm(raw: String): Boolean {
        val text = norm(raw)
        if (text.isEmpty() || text.endsWith("?")) return false
        return text == "start" || text == "start now" || text == "share" || text == "share screen" ||
            text == "allow" || text == "continue" || text == "start sharing" || text == "start casting" ||
            text == "start recording" || text.contains("start now") || text.contains("share screen")
    }

    fun isEntireLabel(raw: String): Boolean {
        val text = norm(raw)
        if (text.isEmpty() || isSingleApp(text)) return false
        return ENTIRE.any { text.contains(it) }
    }

    fun isSingleApp(raw: String): Boolean {
        val text = norm(raw)
        return SINGLE.any { text.contains(it) }
    }

    fun norm(raw: String): String = raw.trim().lowercase().replace(Regex("\\s+"), " ")

    private val ENTIRE = listOf(
        "share entire screen",
        "entire screen",
        "full screen",
        "whole screen",
        "pantalla completa",
        "pantalla entera",
        "tout l'écran",
        "écran entier",
        "ecran entier",
        "gesamten bildschirm",
        "gesamter bildschirm",
        "schermo intero",
        "tela inteira",
        "ecrã inteiro",
        "ecra inteiro",
        "весь экран",
        "整个屏幕",
        "共享整个屏幕",
        "全屏",
        "画面全体",
        "전체 화면",
    )

    private val SINGLE = listOf(
        "single app",
        "one app",
        "a single",
        "this app",
        "una sola",
        "une seule",
        "einzelne app",
        "单个应用",
        "单个应用程序",
    )
}
