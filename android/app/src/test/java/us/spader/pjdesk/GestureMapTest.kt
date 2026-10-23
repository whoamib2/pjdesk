package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.input.GestureMap
import us.spader.pjdesk.input.PictureView
import us.spader.pjdesk.input.PointerSession
import us.spader.pjdesk.input.StrokeTracker

class GestureMapTest {
    @Test
    fun portraitPictureInALandscapeViewIsLetterboxed() {
        val placed = GestureMap.letterbox(1080, 2400, 1920, 1080)
        assertTrue(placed.x > 0)
        assertEquals(0, placed.y)
        assertTrue(placed.width < 1920)
        assertNull(GestureMap.viewToNorm(placed, 2f, 10f))
        val center = GestureMap.viewToNorm(placed, 960f, 540f)
        assertTrue(center != null)
        assertTrue(center!!.first in 30000..36000)
        assertTrue(center.second in 30000..36000)
    }

    @Test
    fun landscapePictureInAPortraitViewUsesTheWidth() {
        val placed = GestureMap.letterbox(1920, 1080, 1080, 1920)
        assertEquals(0, placed.x)
        assertTrue(placed.y > 0)
        val start = GestureMap.viewToNorm(placed, placed.x.toFloat(), placed.y + 4f)
        val end = GestureMap.viewToNorm(placed, (placed.x + placed.width - 1).toFloat(), placed.y + 4f)
        assertEquals(0, start?.first)
        assertEquals(65535, end?.first)
    }

    @Test
    fun rotationSwapsTheCorners() {
        assertEquals(0 to 65535, GestureMap.rotateCw(0, 0))
        assertEquals(0 to 0, GestureMap.rotateCw(65535, 0))
        assertEquals(65535 to 65535, GestureMap.rotateCw(0, 65535))
        var p = 12345 to 40000
        repeat(4) { p = GestureMap.rotateCw(p.first, p.second) }
        assertEquals(12345 to 40000, p)
        val turned = GestureMap.rotateCw(12345, 40000)
        assertEquals(12345 to 40000, GestureMap.rotateCcw(turned.first, turned.second))
    }

    @Test
    fun aDragAcrossThePictureIsTheFullAxisNotTheBars() {
        val placed = GestureMap.letterbox(1080, 2400, 1920, 1080)
        val across = GestureMap.viewDeltaToNorm(placed, placed.width - 1f, 0f)
        assertTrue(kotlin.math.abs(across.first - 65535) < 2)
        val onPicture = kotlin.math.abs(GestureMap.viewDeltaToNorm(placed, 10f, 0f).first)
        val onView = kotlin.math.round(10.0 / (1920.0 - 1.0) * 65535.0).toInt()
        assertTrue(onPicture > onView * 2)
    }

    @Test
    fun clickIsATapAndADragIsASwipe() {
        val t = StrokeTracker()
        t.down(1_000, 1000, 2000)
        t.moveTo(1100, 2050)
        val tap = t.up(1_100)!!
        assertEquals(GestureMap.TAP, tap.kind)
        assertEquals(1000, tap.x[0])

        t.down(2_000, 1000, 2000)
        t.moveTo(1000, 2000 + GestureMap.MOVE_THRESHOLD + 50)
        val swipe = t.up(2_200)!!
        assertEquals(GestureMap.SWIPE, swipe.kind)
        assertTrue(swipe.y[1] > swipe.y[0])

        t.down(3_000, 4000, 4000)
        assertNull(t.pollLongPress(3_200))
        val long = t.pollLongPress(3_000 + GestureMap.LONG_PRESS_MS)!!
        assertEquals(GestureMap.LONG_PRESS, long.kind)
        assertNull(t.up(3_000 + GestureMap.LONG_PRESS_MS + 20))
    }

    @Test
    fun normEdgesRoundTripThroughPixels() {
        assertEquals(0, GestureMap.normToPixel(0, 1080))
        assertEquals(1079, GestureMap.normToPixel(65535, 1080))
        assertEquals(0, GestureMap.normToPixel(0, 2400))
        assertEquals(2399, GestureMap.normToPixel(65535, 2400))
    }

    @Test
    fun androidTouchBecomesAGestureAndWindowsTouchMovesACursor() {
        val phone = PointerSession(android = true)
        phone.down(0, listOf(1000 to 2000))
        phone.move(listOf(1000 to 2000 + GestureMap.MOVE_THRESHOLD + 10))
        val swipe = phone.up(200).filterIsInstance<PointerSession.Out.Gesture>().single()
        assertEquals(GestureMap.SWIPE, swipe.gesture.kind)

        val desktop = PointerSession(android = false)
        desktop.down(0, listOf(1000 to 1000))
        val moved = desktop.move(listOf((1000 + GestureMap.MOVE_THRESHOLD + 5) to 1000))
        assertTrue(moved.any { it is PointerSession.Out.Mouse && it.kind == "down" })
        assertTrue(moved.any { it is PointerSession.Out.Mouse && it.kind == "move" })
        val release = desktop.up(300)
        assertEquals("up", (release.single() as PointerSession.Out.Mouse).kind)

        val click = PointerSession(android = false)
        click.down(0, listOf(4000 to 4000))
        val pressed = click.up(40)
        assertEquals(listOf("down", "up"), pressed.map { (it as PointerSession.Out.Mouse).kind })
    }

    @Test
    fun twoFingerTapIsALongPressAndThreeFingersAreMiddle() {
        val phone = PointerSession(android = true)
        phone.down(0, listOf(1000 to 1000, 1200 to 1200))
        val right = phone.up(80).filterIsInstance<PointerSession.Out.Gesture>().single()
        assertEquals(GestureMap.LONG_PRESS, right.gesture.kind)

        val pad = PointerSession(android = false)
        pad.down(0, listOf(1 to 1, 2 to 2, 3 to 3))
        val middle = pad.up(40).filterIsInstance<PointerSession.Out.Mouse>()
        assertEquals(listOf("down", "up"), middle.map { it.kind })
        assertTrue(middle.all { it.button == "middle" })
    }

    @Test
    fun fitScaleMapsTheCenterAndZoomKeepsItUnderTheFinger() {
        val center = PictureView.screenToNorm(1920, 1080, 1920, 1080, PictureView.fit(), 960f, 540f)
        assertTrue(center!!.first in 32000..33500)
        val zoomed = PictureView.zoomAbout(1920, 1080, 1920, 1080, PictureView.fit(), 960f, 540f, 2f)
        assertTrue(zoomed.scale > 1.5f)
        val again = PictureView.screenToNorm(1920, 1080, 1920, 1080, zoomed, 960f, 540f)
        assertTrue(again!!.first in 32000..33500)
        assertEquals(1f, PictureView.fit().scale, 0.01f)
        assertTrue(PictureView.oneToOne(1920, 480) > 3f)
    }

    @Test
    fun pinchUsesFourPoints() {
        val phone = PointerSession(android = true)
        phone.down(0, listOf(1000 to 1000, 4000 to 4000))
        phone.move(listOf(500 to 500, 8000 to 8000))
        val pinch = phone.up(200).filterIsInstance<PointerSession.Out.Gesture>().single()
        assertEquals(GestureMap.PINCH, pinch.gesture.kind)
        assertEquals(1000, pinch.gesture.x[0])
        assertEquals(8000, pinch.gesture.x[3])
    }

    @Test
    fun permanentPasswordMatchesTheWindowsRule() {
        assertTrue(!us.spader.pjdesk.core.permanentOk("short"))
        assertTrue(!us.spader.pjdesk.core.permanentOk("1234567"))
        assertTrue(us.spader.pjdesk.core.permanentOk("12345678"))
        assertTrue(us.spader.pjdesk.core.permanentOk("  a longer phrase  "))
        assertTrue(!us.spader.pjdesk.core.permanentOk("        "))
        assertEquals("Use at least 8 characters.", us.spader.pjdesk.core.permanentIssue("short", "short"))
        assertEquals("Those passwords do not match.", us.spader.pjdesk.core.permanentIssue("12345678", "12345679"))
        assertNull(us.spader.pjdesk.core.permanentIssue("12345678", "12345678"))
    }
}
