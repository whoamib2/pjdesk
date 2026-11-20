package us.spader.pjdesk.input

/**
 * Controller pointer.
 *
 * Touch on an Android remote sends taps, swipes, and long-presses. A two-finger
 * tap with almost no movement is a long-press (right-click). A two-finger drag
 * is still a pinch for callers that ask for it; the session view uses that
 * motion to zoom the picture instead.
 *
 * Trackpad mode moves a cursor. A short tap clicks. A one-finger drag only
 * moves the pointer — never a press-and-hold. A double-tap-and-drag, or a
 * still long-press of [GestureMap.LONG_PRESS_MS] inside the slop, may start
 * a drag. Two fingers tap for a right-click, slide to scroll, and three
 * fingers click the middle button. [speed] scales that movement (0.6 is the
 * default). [accelerate] lengthens a fast flick.
 *
 * Touch mode on a desktop remote clicks where the finger is. A long-press or a
 * two-finger tap is the right button. A drag holds the left button. Callers
 * that pan a zoomed picture should not send those moves here.
 */
class PointerSession(
    val android: Boolean,
    /** Null keeps the previous split: Android remotes use gestures, desktops use the trackpad. */
    trackpad: Boolean? = null,
    /** Multiplier for trackpad movement. 1 matches the previous speed. */
    var speed: Float = 0.6f,
    /** When on, a faster finger flick moves the pointer further. */
    var accelerate: Boolean = false,
    /** When on, a one-finger trackpad drag holds the mouse button. */
    var dragLock: Boolean = false,
    /** Multiplier for a two-finger wheel notch. 1 matches the previous wheel. */
    var scrollSpeed: Float = 1f,
    /** Flips the wheel direction. */
    var invertScroll: Boolean = false,
) {
    private val relative = trackpad ?: !android
    private val stroke = StrokeTracker()
    private var cursor = GestureMap.NORM / 2 to GestureMap.NORM / 2
    private var last: Pair<Int, Int>? = null
    private var origin: Pair<Int, Int>? = null
    private var pinchStart: Pair<Pair<Int, Int>, Pair<Int, Int>>? = null
    private var pinchLast: Pair<Pair<Int, Int>, Pair<Int, Int>>? = null
    private var twoStart: Pair<Pair<Int, Int>, Pair<Int, Int>>? = null
    private var twoLast: Pair<Pair<Int, Int>, Pair<Int, Int>>? = null
    private var scrollAccX = 0
    private var scrollAccY = 0
    private var dragging = false
    private var middle = false
    private var twoFinger = false
    private var twoMoved = false
    private var rightSent = false
    private var downMs = 0L
    /** Finger travel past the move threshold, without a button necessarily held. */
    private var moved = false
    /** Set by a tap-then-press so the next drag holds the button. */
    private var armed = false
    /** The stroke that just ended was a short motionless tap. */
    var lastWasTap: Boolean = false
        private set

    val cursorNorm: Pair<Int, Int> get() = cursor
    val dragArmed: Boolean get() = armed

    /** The next one-finger press-and-drag holds the mouse button. */
    fun armDrag() {
        armed = true
    }

    fun down(tMs: Long, points: List<Pair<Int, Int>>) {
        if (points.isEmpty()) return
        lastWasTap = false
        if (relative) {
            if (points.size >= 3) {
                middle = true
                twoFinger = false
                twoStart = null
                origin = null
                return
            }
            if (points.size >= 2) {
                twoFinger = true
                twoMoved = false
                twoStart = points[0] to points[1]
                twoLast = twoStart
                downMs = tMs
                scrollAccX = 0
                scrollAccY = 0
                origin = null
                return
            }
            origin = points[0]
            last = points[0]
            dragging = false
            moved = false
            twoFinger = false
            downMs = tMs
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
            if (points.size >= 2 || twoFinger) {
                return moveTwoFinger(points)
            }
            val from = last ?: points[0]
            val dx = points[0].first - from.first
            val dy = points[0].second - from.second
            last = points[0]
            val out = ArrayList<Out>(2)
            val start = origin
            if (start != null && chebyshev(start, points[0]) >= GestureMap.MOVE_THRESHOLD) {
                moved = true
                // Movement past slop cancels a pending long-press. A drag
                // starts only when the user armed a double-tap-and-drag, or
                // asked for drag-lock, or already held after a long-press.
                if (!dragging && (dragLock || armed)) {
                    dragging = true
                    out.add(Out.Mouse("down", cursor.first, cursor.second, 0))
                }
            }
            val (sx, sy) = scaleDelta(dx, dy)
            cursor = GestureMap.trackpadMove(cursor, sx, sy)
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
                twoStart = null
                twoFinger = false
                dragging = false
                lastWasTap = false
                return listOf(
                    Out.Mouse("down", cursor.first, cursor.second, 0, "middle"),
                    Out.Mouse("up", cursor.first, cursor.second, 0, "middle"),
                )
            }
            if (twoFinger) {
                val start = twoStart
                val travelled = twoMoved
                val held = tMs - downMs
                twoFinger = false
                twoStart = null
                twoLast = null
                twoMoved = false
                scrollAccX = 0
                scrollAccY = 0
                lastWasTap = false
                if (start != null && !travelled && held <= GestureMap.TWO_FINGER_TAP_MS) {
                    return listOf(
                        Out.Mouse("down", cursor.first, cursor.second, 0, "right"),
                        Out.Mouse("up", cursor.first, cursor.second, 0, "right"),
                    )
                }
                return emptyList()
            }
            val had = origin
            origin = null
            last = null
            if (had == null) {
                lastWasTap = false
                return emptyList()
            }
            val held = dragging
            val travelled = moved
            dragging = false
            moved = false
            armed = false
            lastWasTap = !held && !travelled
            return when {
                held -> listOf(Out.Mouse("up", cursor.first, cursor.second, 0))
                travelled -> emptyList()
                else -> listOf(
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
        if (relative) {
            if (!dragging && origin != null && !moved && tMs - downMs >= GestureMap.LONG_PRESS_MS) {
                dragging = true
                armed = false
                return Out.Mouse("down", cursor.first, cursor.second, 0)
            }
            return null
        }
        if (android && !relative) {
            val g = stroke.pollLongPress(tMs) ?: return null
            return Out.Gesture(g)
        }
        if (!rightSent && !dragging && origin != null && tMs - downMs >= GestureMap.LONG_PRESS_MS) {
            rightSent = true
            val at = origin!!
            return Out.Mouse("down", at.first, at.second, 0, "right")
        }
        return null
    }

    /** Drops the gesture. A held button is released so a pan cannot leave the mouse down. */
    fun cancel(): List<Out> {
        val release = if (dragging) {
            listOf(Out.Mouse("up", cursor.first, cursor.second, 0))
        } else {
            emptyList()
        }
        stroke.cancel()
        pinchStart = null
        pinchLast = null
        twoStart = null
        twoLast = null
        origin = null
        last = null
        scrollAccX = 0
        scrollAccY = 0
        dragging = false
        moved = false
        armed = false
        middle = false
        twoFinger = false
        twoMoved = false
        rightSent = false
        return release
    }

    private fun moveTwoFinger(points: List<Pair<Int, Int>>): List<Out> {
        if (points.size < 2) return emptyList()
        val now = points[0] to points[1]
        val start = twoStart ?: now.also { twoStart = it }
        val prev = twoLast ?: start
        twoLast = now
        twoFinger = true
        val prevCx = (prev.first.first + prev.second.first) / 2
        val prevCy = (prev.first.second + prev.second.second) / 2
        val nowCx = (now.first.first + now.second.first) / 2
        val nowCy = (now.first.second + now.second.second) / 2
        scrollAccX += nowCx - prevCx
        scrollAccY += nowCy - prevCy
        val travel = maxOf(
            chebyshev(start.first, now.first),
            chebyshev(start.second, now.second),
        )
        if (travel < GestureMap.SCROLL_SLOP) return emptyList()
        twoMoved = true
        val speed = scrollSpeed.coerceIn(0.25f, 4f)
        val units = (GestureMap.SCROLL_UNITS_PER_NOTCH / speed).toInt().coerceIn(200, 8000)
        var notchesY = scrollAccY / units
        var notchesX = scrollAccX / units
        scrollAccY -= notchesY * units
        scrollAccX -= notchesX * units
        // Fingers down (positive Android y) → negative wheel, same as before.
        if (invertScroll) {
            notchesY = -notchesY
            notchesX = -notchesX
        }
        val dy = -notchesY * 120
        val dx = -notchesX * 120
        if (dx == 0 && dy == 0) return emptyList()
        return listOf(Out.Mouse("wheel", cursor.first, cursor.second, dy, dx = dx))
    }

    private fun scaleDelta(dx: Int, dy: Int): Pair<Int, Int> {
        if (!relative) return dx to dy
        var gain = speed.coerceIn(0.25f, 2f)
        if (accelerate) {
            val mag = kotlin.math.hypot(dx.toDouble(), dy.toDouble()).toFloat()
            gain *= 1f + (mag / 8000f).coerceAtMost(1.5f)
        }
        return (dx * gain).let { kotlin.math.round(it.toDouble()).toInt() } to
            (dy * gain).let { kotlin.math.round(it.toDouble()).toInt() }
    }

    private fun chebyshev(a: Pair<Int, Int>, b: Pair<Int, Int>): Int =
        maxOf(kotlin.math.abs(a.first - b.first), kotlin.math.abs(a.second - b.second))

    sealed class Out {
        data class Gesture(val gesture: GestureMap.Gesture) : Out()
        data class Mouse(val kind: String, val x: Int, val y: Int, val dy: Int, val button: String = "left", val dx: Int = 0) : Out()
    }
}
