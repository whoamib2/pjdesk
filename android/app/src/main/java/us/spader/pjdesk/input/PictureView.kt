package us.spader.pjdesk.input

/**
 * Local zoom and pan of the remote picture. Scale 1 is fit-to-screen.
 * Points are mapped through the same letterbox as [GestureMap], then scaled
 * around the middle of the view.
 */
object PictureView {
    data class Frame(val scale: Float, val panX: Float, val panY: Float)

    fun fit() = Frame(1f, 0f, 0f)

    /** Remote pixels to view pixels. 1:1 when the picture is larger than the fit rect. */
    fun oneToOne(frameW: Int, fitWidth: Int): Float {
        if (frameW <= 0 || fitWidth <= 0) return 1f
        return (frameW.toFloat() / fitWidth).coerceIn(1f, 8f)
    }

    fun screenToNorm(
        viewW: Int,
        viewH: Int,
        frameW: Int,
        frameH: Int,
        frame: Frame,
        x: Float,
        y: Float,
    ): Pair<Int, Int>? {
        val placed = GestureMap.letterbox(frameW, frameH, viewW, viewH)
        if (placed.width < 2 || placed.height < 2) return null
        val dw = placed.width * frame.scale
        val dh = placed.height * frame.scale
        val left = viewW / 2f + frame.panX - dw / 2f
        val top = viewH / 2f + frame.panY - dh / 2f
        if (x < left || y < top || x >= left + dw || y >= top + dh) return null
        val nx = ((x - left) / dw * GestureMap.NORM).toDouble().let { kotlin.math.round(it) }
        val ny = ((y - top) / dh * GestureMap.NORM).toDouble().let { kotlin.math.round(it) }
        return nx.coerceIn(0.0, GestureMap.NORM.toDouble()).toInt() to
            ny.coerceIn(0.0, GestureMap.NORM.toDouble()).toInt()
    }

    /** Keeps the remote point under the fingers when the scale changes. */
    fun zoomAbout(
        viewW: Int,
        viewH: Int,
        frameW: Int,
        frameH: Int,
        frame: Frame,
        focusX: Float,
        focusY: Float,
        newScale: Float,
    ): Frame {
        val placed = GestureMap.letterbox(frameW, frameH, viewW, viewH)
        if (placed.width < 2 || placed.height < 2) return frame
        val scale = newScale.coerceIn(1f, 8f)
        val before = screenToNorm(viewW, viewH, frameW, frameH, frame, focusX, focusY) ?: return frame.copy(scale = scale)
        val dw = placed.width * scale
        val dh = placed.height * scale
        val left = focusX - (before.first / GestureMap.NORM.toFloat()) * dw
        val top = focusY - (before.second / GestureMap.NORM.toFloat()) * dh
        val panX = left + dw / 2f - viewW / 2f
        val panY = top + dh / 2f - viewH / 2f
        return clamp(viewW, viewH, placed.width, placed.height, Frame(scale, panX, panY))
    }

    fun clamp(viewW: Int, viewH: Int, fitW: Int, fitH: Int, frame: Frame): Frame {
        if (frame.scale <= 1.01f) return Frame(1f, 0f, 0f)
        val extraX = (fitW * frame.scale - viewW) / 2f
        val extraY = (fitH * frame.scale - viewH) / 2f
        val panX = if (extraX <= 0f) 0f else frame.panX.coerceIn(-extraX, extraX)
        val panY = if (extraY <= 0f) 0f else frame.panY.coerceIn(-extraY, extraY)
        return Frame(frame.scale, panX, panY)
    }

    /** Shifts the pan so a zoomed cursor stays inside the view. */
    fun followCursor(
        viewW: Int,
        viewH: Int,
        frameW: Int,
        frameH: Int,
        frame: Frame,
        cursor: Pair<Int, Int>,
    ): Frame {
        if (frame.scale <= 1.01f) return frame
        val placed = GestureMap.letterbox(frameW, frameH, viewW, viewH)
        if (placed.width < 2 || placed.height < 2) return frame
        val dw = placed.width * frame.scale
        val dh = placed.height * frame.scale
        var left = viewW / 2f + frame.panX - dw / 2f
        var top = viewH / 2f + frame.panY - dh / 2f
        val cx = left + (cursor.first / GestureMap.NORM.toFloat()) * dw
        val cy = top + (cursor.second / GestureMap.NORM.toFloat()) * dh
        val marginX = viewW * 0.2f
        val marginY = viewH * 0.2f
        if (cx < marginX) left += marginX - cx
        if (cx > viewW - marginX) left -= cx - (viewW - marginX)
        if (cy < marginY) top += marginY - cy
        if (cy > viewH - marginY) top -= cy - (viewH - marginY)
        val panX = left + dw / 2f - viewW / 2f
        val panY = top + dh / 2f - viewH / 2f
        return clamp(viewW, viewH, placed.width, placed.height, frame.copy(panX = panX, panY = panY))
    }
}
