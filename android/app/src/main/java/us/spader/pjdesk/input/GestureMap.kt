package us.spader.pjdesk.input

/**
 * Coordinate and gesture mapping shared with `crates/pjdesk-proto/src/gesture.rs`.
 * The formulas are the same: a picture is letterboxed in the view, bars are not
 * input, and points on the picture are normalized to `0..=65535`.
 */
object GestureMap {
    const val NORM: Int = 65535
    const val TAP: Int = 0
    const val LONG_PRESS: Int = 1
    const val SWIPE: Int = 2
    const val PINCH: Int = 3
    const val MOVE_THRESHOLD: Int = 800
    const val LONG_PRESS_MS: Long = 500

    data class Letterbox(val x: Int, val y: Int, val width: Int, val height: Int)

    data class Gesture(
        val kind: Int,
        val durationMs: Int,
        val x: IntArray,
        val y: IntArray,
    )

    fun letterbox(frameW: Int, frameH: Int, viewW: Int, viewH: Int): Letterbox {
        if (frameW <= 0 || frameH <= 0 || viewW <= 0 || viewH <= 0) {
            return Letterbox(0, 0, 0, 0)
        }
        val (width, height) = if (frameW.toLong() * viewH <= viewW.toLong() * frameH) {
            val h = viewH
            val w = ((frameW.toLong() * h) / frameH).coerceIn(1, viewW.toLong()).toInt()
            w to h
        } else {
            val w = viewW
            val h = ((frameH.toLong() * w) / frameW).coerceIn(1, viewH.toLong()).toInt()
            w to h
        }
        return Letterbox(
            x = (viewW - width) / 2,
            y = (viewH - height) / 2,
            width = width,
            height = height,
        )
    }

    fun viewToNorm(placed: Letterbox, x: Float, y: Float): Pair<Int, Int>? {
        if (placed.width < 2 || placed.height < 2) return null
        val right = placed.x + placed.width.toFloat()
        val bottom = placed.y + placed.height.toFloat()
        if (x < placed.x || y < placed.y || x >= right || y >= bottom) return null
        val nx = ((x - placed.x) / (placed.width - 1f) * NORM).toDouble().let { kotlin.math.round(it) }
        val ny = ((y - placed.y) / (placed.height - 1f) * NORM).toDouble().let { kotlin.math.round(it) }
        return nx.coerceIn(0.0, NORM.toDouble()).toInt() to ny.coerceIn(0.0, NORM.toDouble()).toInt()
    }

    fun viewDeltaToNorm(placed: Letterbox, dx: Float, dy: Float): Pair<Int, Int> {
        if (placed.width < 2 || placed.height < 2) return 0 to 0
        val sx = kotlin.math.round((dx / (placed.width - 1f) * NORM).toDouble()).toInt()
        val sy = kotlin.math.round((dy / (placed.height - 1f) * NORM).toDouble()).toInt()
        return sx to sy
    }

    fun normToPixel(n: Int, size: Int): Int {
        if (size <= 1) return 0
        val value = (n.toLong() * (size.toLong() - 1) + 32767) / NORM
        return value.coerceAtMost(size.toLong() - 1).toInt()
    }

    /** Top-left `(0, 0)` becomes the top-right of a buffer rotated clockwise. */
    fun rotateCw(x: Int, y: Int): Pair<Int, Int> = y to (NORM - x)

    fun rotateCcw(x: Int, y: Int): Pair<Int, Int> = (NORM - y) to x

    fun tap(x: Int, y: Int) = Gesture(TAP, 50, intArrayOf(x, 0, 0, 0), intArrayOf(y, 0, 0, 0))

    fun longPress(x: Int, y: Int, durationMs: Int) = Gesture(
        LONG_PRESS,
        durationMs.coerceAtLeast(LONG_PRESS_MS.toInt()),
        intArrayOf(x, 0, 0, 0),
        intArrayOf(y, 0, 0, 0),
    )

    fun swipe(x0: Int, y0: Int, x1: Int, y1: Int, durationMs: Int) = Gesture(
        SWIPE,
        durationMs.coerceIn(80, 1200),
        intArrayOf(x0, x1, 0, 0),
        intArrayOf(y0, y1, 0, 0),
    )

    fun pinch(
        a0: Pair<Int, Int>,
        a1: Pair<Int, Int>,
        b0: Pair<Int, Int>,
        b1: Pair<Int, Int>,
        durationMs: Int,
    ) = Gesture(
        PINCH,
        durationMs.coerceIn(80, 1200),
        intArrayOf(a0.first, a1.first, b0.first, b1.first),
        intArrayOf(a0.second, a1.second, b0.second, b1.second),
    )

    fun wheelSwipe(x: Int, y: Int, notches: Int): Gesture {
        val dy = (notches * 6000).coerceIn(-20000, 20000)
        val y1 = (y + dy).coerceIn(0, NORM)
        return swipe(x, y, x, y1, 180)
    }

    fun trackpadMove(cursor: Pair<Int, Int>, dx: Int, dy: Int): Pair<Int, Int> {
        val x = (cursor.first + dx).coerceIn(0, NORM)
        val y = (cursor.second + dy).coerceIn(0, NORM)
        return x to y
    }
}

class StrokeTracker {
    var down: Boolean = false
        private set
    private var downMs: Long = 0
    private var origin: Pair<Int, Int> = 0 to 0
    private var last: Pair<Int, Int> = 0 to 0
    private var moved: Boolean = false
    private var fired: Boolean = false

    fun down(tMs: Long, x: Int, y: Int) {
        down = true
        downMs = tMs
        origin = x to y
        last = x to y
        moved = false
        fired = false
    }

    fun moveTo(x: Int, y: Int) {
        if (!down) {
            last = x to y
            return
        }
        last = x to y
        if (dist(origin, x to y) >= GestureMap.MOVE_THRESHOLD) moved = true
    }

    fun pollLongPress(tMs: Long): GestureMap.Gesture? {
        if (!down || moved || fired) return null
        if (tMs - downMs < GestureMap.LONG_PRESS_MS) return null
        fired = true
        return GestureMap.longPress(origin.first, origin.second, GestureMap.LONG_PRESS_MS.toInt())
    }

    fun up(tMs: Long): GestureMap.Gesture? {
        if (!down) return null
        down = false
        if (fired) return null
        val held = tMs - downMs
        if (moved) {
            val duration = (120 + dist(origin, last) / 40).coerceIn(120, 700)
            return GestureMap.swipe(origin.first, origin.second, last.first, last.second, duration)
        }
        if (held >= GestureMap.LONG_PRESS_MS) {
            return GestureMap.longPress(origin.first, origin.second, held.coerceAtMost(65535).toInt())
        }
        return GestureMap.tap(origin.first, origin.second)
    }

    fun cancel() {
        down = false
        fired = false
        moved = false
    }

    private fun dist(a: Pair<Int, Int>, b: Pair<Int, Int>): Int {
        val dx = kotlin.math.abs(a.first - b.first)
        val dy = kotlin.math.abs(a.second - b.second)
        return maxOf(dx, dy)
    }
}
