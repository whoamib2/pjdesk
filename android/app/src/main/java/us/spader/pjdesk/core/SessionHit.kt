package us.spader.pjdesk.core

/** A control rectangle in the session view's coordinate space. */
data class ControlPad(
    val left: Float,
    val top: Float,
    val right: Float,
    val bottom: Float,
)

/**
 * Hit rules for the session overlay.
 *
 * The 0.6.13 menu bubble used `detectDragGestures` and toggled itself from
 * `onDragEnd` only when the finger had moved less than 12px. That callback
 * runs after the touch slop. On a Pixel the slop is larger than 12px, so a
 * tap never opened the menu and a drag only moved the bubble.
 */
object SessionHit {
    fun covered(x: Float, y: Float, pads: Collection<ControlPad>): Boolean =
        pads.any { x >= it.left && y >= it.top && x < it.right && y < it.bottom }

    /** A finger that lifts before the touch slop is a tap. */
    fun tapOpens(movedPx: Float, slopPx: Float): Boolean = movedPx < slopPx

    /** The 0.6.13 rule. True only for a drag that passed the slop and stayed under 12px. */
    fun legacyDragOpens(movedPx: Float, slopPx: Float, openThresholdPx: Float = 12f): Boolean =
        movedPx >= slopPx && movedPx < openThresholdPx
}
