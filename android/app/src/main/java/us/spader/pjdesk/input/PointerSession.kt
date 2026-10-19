package us.spader.pjdesk.input

/**
 * Controller pointer. An Android remote gets taps, swipes, long-press, and
 * pinch. A Windows (or other desktop) remote gets a trackpad: movement moves
 * a cursor, a short tap clicks, a drag holds the button, and a two-finger
 * drag scrolls.
 */
class PointerSession(val android: Boolean) {
    private val stroke = StrokeTracker()
    private var cursor = GestureMap.NORM / 2 to GestureMap.NORM / 2
    private var last: Pair<Int, Int>? = null
    private var origin: Pair<Int, Int>? = null
    private var pinchStart: Pair<Pair<Int, Int>, Pair<Int, Int>>? = null
    private var pinchLast: Pair<Pair<Int, Int>, Pair<Int, Int>>? = null
    private var scrollY: Int? = null
    private var dragging = false

    val cursorNorm: Pair<Int, Int> get() = cursor

    fun down(tMs: Long, points: List<Pair<Int, Int>>) {
        if (points.isEmpty()) return
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
            scrollY = points[0].second
            return
        }
        origin = points[0]
        last = points[0]
        dragging = false
    }

    fun move(points: List<Pair<Int, Int>>): List<Out> {
        if (points.isEmpty()) return emptyList()
        if (android) {
            if (points.size >= 2 && pinchStart != null) {
                pinchLast = points[0] to points[1]
            } else {
                last = points[0]
                stroke.moveTo(points[0].first, points[0].second)
            }
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

    fun up(tMs: Long): List<Out> {
        if (android && pinchStart != null && pinchLast != null) {
            val start = pinchStart!!
            val end = pinchLast!!
            pinchStart = null
            pinchLast = null
            return listOf(Out.Gesture(GestureMap.pinch(start.first, end.first, start.second, end.second, 220)))
        }
        pinchStart = null
        scrollY = null
        if (android) {
            val g = stroke.up(tMs) ?: return emptyList()
            return listOf(Out.Gesture(g))
        }
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

    fun poll(tMs: Long): Out? {
        val g = stroke.pollLongPress(tMs) ?: return null
        return Out.Gesture(g)
    }

    fun cancel() {
        stroke.cancel()
        pinchStart = null
        pinchLast = null
        origin = null
        last = null
        scrollY = null
        dragging = false
    }

    private fun chebyshev(a: Pair<Int, Int>, b: Pair<Int, Int>): Int =
        maxOf(kotlin.math.abs(a.first - b.first), kotlin.math.abs(a.second - b.second))

    sealed class Out {
        data class Gesture(val gesture: GestureMap.Gesture) : Out()
        data class Mouse(val kind: String, val x: Int, val y: Int, val dy: Int) : Out()
    }
}
