package us.spader.pjdesk.service

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.graphics.Path
import android.graphics.Point
import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import android.view.accessibility.AccessibilityNodeInfo
import org.json.JSONObject
import us.spader.pjdesk.a11y.PjAccessibilityService
import us.spader.pjdesk.input.GestureMap

/**
 * Applies remote input on this phone. Gestures go through the accessibility
 * service. Volume uses [android.media.AudioManager] because a global action
 * does not change the media stream.
 */
object RemoteControl {
    @Volatile
    var service: PjAccessibilityService? = null

    fun handle(event: JSONObject) {
        val svc = service ?: return
        when (event.optString("kind")) {
            "text" -> svc.injectText(event.optString("chars"))
            "special" -> svc.special(event.optInt("which"))
            "gesture" -> svc.gesture(event)
        }
    }
}

fun screenSize(service: AccessibilityService): Pair<Int, Int> {
    val wm = service.getSystemService(WindowManager::class.java)
    if (Build.VERSION.SDK_INT >= 30) {
        val b = wm.currentWindowMetrics.bounds
        return b.width() to b.height()
    }
    val point = Point()
    @Suppress("DEPRECATION")
    wm.defaultDisplay.getRealSize(point)
    return point.x to point.y
}

fun gestureDescription(event: JSONObject, width: Int, height: Int): GestureDescription? {
    val kind = event.optInt("gesture")
    val duration = event.optLong("duration").coerceIn(1, 5000)
    val xs = event.optJSONArray("x") ?: return null
    val ys = event.optJSONArray("y") ?: return null
    fun pt(i: Int): Pair<Float, Float> {
        val x = GestureMap.normToPixel(xs.optInt(i), width).toFloat()
        val y = GestureMap.normToPixel(ys.optInt(i), height).toFloat()
        return x to y
    }
    val builder = GestureDescription.Builder()
    when (kind) {
        GestureMap.TAP, GestureMap.LONG_PRESS -> {
            val (x, y) = pt(0)
            val path = Path().apply { moveTo(x, y) }
            val held = if (kind == GestureMap.LONG_PRESS) duration.coerceAtLeast(GestureMap.LONG_PRESS_MS) else duration.coerceAtLeast(40)
            builder.addStroke(GestureDescription.StrokeDescription(path, 0, held))
        }
        GestureMap.SWIPE -> {
            val (x0, y0) = pt(0)
            val (x1, y1) = pt(1)
            val path = Path().apply {
                moveTo(x0, y0)
                lineTo(x1, y1)
            }
            builder.addStroke(GestureDescription.StrokeDescription(path, 0, duration.coerceAtLeast(80)))
        }
        GestureMap.PINCH -> {
            val (ax0, ay0) = pt(0)
            val (ax1, ay1) = pt(1)
            val (bx0, by0) = pt(2)
            val (bx1, by1) = pt(3)
            val a = Path().apply {
                moveTo(ax0, ay0)
                lineTo(ax1, ay1)
            }
            val b = Path().apply {
                moveTo(bx0, by0)
                lineTo(bx1, by1)
            }
            val held = duration.coerceAtLeast(80)
            builder.addStroke(GestureDescription.StrokeDescription(a, 0, held))
            builder.addStroke(GestureDescription.StrokeDescription(b, 0, held))
        }
        else -> return null
    }
    return builder.build()
}

fun setFocusedText(root: AccessibilityNodeInfo?, chars: String): Boolean {
    val node = root?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return false
    val current = node.text?.toString() ?: ""
    val next = when {
        chars == "\u0008" -> current.dropLast(1)
        chars == "\n" -> {
            // ACTION_IME_ENTER is 0x00200002. A field that does not handle it
            // gets a newline instead.
            if (!node.performAction(0x00200002)) {
                val args = Bundle()
                args.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, current + "\n")
                node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
            }
            return true
        }
        else -> current + chars
    }
    val args = Bundle()
    args.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, next)
    return node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
}
