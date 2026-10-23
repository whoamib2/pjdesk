package us.spader.pjdesk.service

import kotlin.math.abs

/**
 * Android 14 and later report the captured region's size. The whole display
 * matches the physical pixels in either orientation. A single app is smaller:
 * it loses the status bar, or it is only a window. A few pixels of slop covers
 * rounding. An unknown size is not treated as a single app.
 */
object EntireDisplay {
    fun isSingleApp(capturedW: Int, capturedH: Int, displayW: Int, displayH: Int): Boolean {
        if (capturedW <= 1 || capturedH <= 1 || displayW <= 1 || displayH <= 1) return false
        return !covers(capturedW, capturedH, displayW, displayH) &&
            !covers(capturedW, capturedH, displayH, displayW)
    }

    private fun covers(capturedW: Int, capturedH: Int, displayW: Int, displayH: Int): Boolean {
        return near(capturedW, displayW) && near(capturedH, displayH)
    }

    /** 16px, or 1% of that side, whichever is larger. A status bar is more than that. */
    private fun near(got: Int, expect: Int): Boolean {
        val slop = maxOf(16, expect / 100)
        return abs(got - expect) <= slop
    }
}
