package us.spader.pjdesk.input

/**
 * Controller pointer.
 *
 * Touch on an Android remote sends taps, swipes, and long-presses. A two-finger
 * tap with almost no movement is a long-press (right-click). A two-finger drag
 * is still a pinch for callers that ask for it; the session view uses that
 * motion to zoom the picture instead.
 *
 * Trackpad mode moves a cursor. A short tap clicks, a drag holds the button,
 * two fingers scroll, and three fingers click the middle button.
 *
 * Touch mode on a desktop remote clicks where the finger is. A long-press or a
 * two-finger tap is the right button. A drag holds the left button.
 */
class PointerSession(
    val android: Boolean,
    /** Null keeps the previous split: Android remotes use gestures, desktops use the trackpad. */
    trackpad: Boolean? = null,
) {
    private val relative = trackpad ?: !android
    private val stroke = StrokeTracker()
    private var cursor = GestureMap.NORM / 2 to GestureMap.NORM / 2
    private var last: Pair<Int, Int>? = null
    private var origin: Pair<Int, Int>? = null
    private var pinchStart: Pair<Pair<Int, Int>, Pair<Int, Int>>? = null
    private var pinchLast: Pair<Pair<Int, Int>, Pair<Int, Int>>? = null
    private var scrollY: Int? = null
    private var dragging = false
    private var middle = false
    private var twoFinger = false
    private var rightSent = false
    private var downMs = 0L

    val cursorNorm: Pair<Int, Int> get() = cursor

    fun down(tMs: Long, points: List<Pair<Int, Int>>) {
        if (points.isEmpty()) return
        if (relative) {
            if (points.size >= 3) {
                middle = true
                scrollY = null
                origin = null
                return
            }
            if (points.size >= 2) {
                scrollY = points[0].second
                return
            }
            origin = points[0]
            last = points[0]
            dragging = false
            return
        }
        if (android) {
            if (points.size >= 2) {
                stroke.cancel()
                pinchStart = points[0] to points[1]
                pinchLast = pinchStart
            } else {
                last = points[0]
                stroke.down(tMs, points[0].first, points[0].second)
            }
            return
        }
        if (points.size >= 2) {
            twoFinger = true
            return
        }
        origin = points[0]
        last = points[0]
        dragging = false
        rightSent = false
        downMs = tMs
    }

    fun move(points: List<Pair<Int, Int>>): List<Out> {
        if (points.isEmpty()) return emptyList()
        if (relative) {
            if (points.size >= 3 || middle) {
                middle = true
                return emptyList()
            }
            if (points.size >= 2) {
                val originY = scrollY ?: points[0].second.also { scrollY = it }
                val dy = points[0].second - originY
                if (kotlin.math.abs(dy) < 1200) return emptyList()
                scrollY = points[0].second
                val notches = if (dy > 0) -1 else 1
                return listOf(Out.Mouse("wheel", cursor.first, cursor.second, notches * 120))
            }
            val from = last ?: points[0]
            val dx = points[0].first - from.first
            val dy = points[0].second - from.second
            last = points[0]
            val out = ArrayList<Out>(2)
            val start = origin
            if (!dragging && start != null && chebyshev(start, points[0]) >= GestureMap.MOVE_THRESHOLD) {
                dragging = true
                out.add(Out.Mouse("down", cursor.first, cursor.second, 0))
            }
            cursor = GestureMap.trackpadMove(cursor, dx, dy)
            out.add(Out.Mouse("move", cursor.first, cursor.second, 0))
            return out
        }
        if (android) {
            if (points.size >= 2 && pinchStart != null) {
                pinchLast = points[0] to points[1]
            } else {
                last = points[0]
                stroke.moveTo(points[0].first, points[0].second)
            }
            return emptyList()
        }
        if (twoFinger || points.size >= 2 || rightSent) return emptyList()
        val start = origin ?: return emptyList()
        val out = ArrayList<Out>(2)
        if (!dragging && chebyshev(start, points[0]) >= GestureMap.MOVE_THRESHOLD) {
            dragging = true
            out.add(Out.Mouse("down", start.first, start.second, 0))
        }
        last = points[0]
        if (dragging) out.add(Out.Mouse("move", points[0].first, points[0].second, 0))
        return out
    }

    fun up(tMs: Long): List<Out> {
        if (relative) {
            if (middle) {
                middle = false
                origin = null
                last = null
                scrollY = null
                dragging = false
                return listOf(
                    Out.Mouse("down", cursor.first, cursor.second, 0, "middle"),
                    Out.Mouse("up", cursor.first, cursor.second, 0, "middle"),
                )
            }
            scrollY = null
            val had = origin
            origin = null
            last = null
            if (had == null) return emptyList()
            return if (dragging) {
                dragging = false
                listOf(Out.Mouse("up", cursor.first, cursor.second, 0))
            } else {
                listOf(
                    Out.Mouse("down", cursor.first, cursor.second, 0),
                    Out.Mouse("up", cursor.first, cursor.second, 0),
                )
            }
        }
        if (android && pinchStart != null && pinchLast != null) {
            val start = pinchStart!!
            val end = pinchLast!!
            pinchStart = null
            pinchLast = null
            val moved = maxOf(
                chebyshev(start.first, end.first),
                chebyshev(start.second, end.second),
            )
            if (moved < GestureMap.MOVE_THRESHOLD) {
                return listOf(
                    Out.Gesture(
                        GestureMap.longPress(start.first.first, start.first.second, GestureMap.LONG_PRESS_MS.toInt()),
                    ),
                )
            }
            return listOf(Out.Gesture(GestureMap.pinch(start.first, end.first, start.second, end.second, 220)))
        }
        pinchStart = null
        if (android) {
            val g = stroke.up(tMs) ?: return emptyList()
            return listOf(Out.Gesture(g))
        }
        if (twoFinger) {
            twoFinger = false
            val at = origin ?: last ?: return emptyList()
            origin = null
            return listOf(
                Out.Mouse("down", at.first, at.second, 0, "right"),
                Out.Mouse("up", at.first, at.second, 0, "right"),
            )
        }
        val at = last
        val had = origin
        origin = null
        last = null
        if (at == null || had == null) return emptyList()
        if (rightSent) {
            rightSent = false
            dragging = false
            return listOf(Out.Mouse("up", at.first, at.second, 0, "right"))
        }
        if (dragging) {
            dragging = false
            return listOf(Out.Mouse("up", at.first, at.second, 0))
        }
        val button = if (tMs - downMs >= GestureMap.LONG_PRESS_MS) "right" else "left"
        return listOf(
            Out.Mouse("down", at.first, at.second, 0, button),
            Out.Mouse("up", at.first, at.second, 0, button),
        )
    }

    fun poll(tMs: Long): Out? {
        if (android && !relative) {
            val g = stroke.pollLongPress(tMs) ?: return null
            return Out.Gesture(g)
        }
        if (!relative && !rightSent && !dragging && origin != null && tMs - downMs >= GestureMap.LONG_PRESS_MS) {
            rightSent = true
            val at = origin!!
            return Out.Mouse("down", at.first, at.second, 0, "right")
        }
        return null
    }

    fun cancel() {
        stroke.cancel()
        pinchStart = null
        pinchLast = null
        origin = null
        last = null
        scrollY = null
        dragging = false
        middle = false
        twoFinger = false
        rightSent = false
    }

    private fun chebyshev(a: Pair<Int, Int>, b: Pair<Int, Int>): Int =
        maxOf(kotlin.math.abs(a.first - b.first), kotlin.math.abs(a.second - b.second))

    sealed class Out {
        data class Gesture(val gesture: GestureMap.Gesture) : Out()
        data class Mouse(val kind: String, val x: Int, val y: Int, val dy: Int, val button: String = "left") : Out()
    }
}
