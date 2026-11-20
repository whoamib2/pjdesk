package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.input.GestureMap
import us.spader.pjdesk.input.PointerSession

class PointerSessionTest {
    private fun pad() = PointerSession(android = false, trackpad = true)

    private fun wheelOf(session: PointerSession, from: List<Pair<Int, Int>>, to: List<Pair<Int, Int>>): PointerSession.Out.Mouse? {
        session.down(0, from)
        return session.move(to).filterIsInstance<PointerSession.Out.Mouse>().firstOrNull { it.kind == "wheel" }
    }

    @Test
    fun aShortTapClicksAndAMoveDoesNotPress() {
        val tap = pad()
        tap.down(0, listOf(4000 to 4000))
        val clicked = tap.up(40).filterIsInstance<PointerSession.Out.Mouse>()
        assertEquals(listOf("down", "up"), clicked.map { it.kind })
        assertTrue(tap.lastWasTap)

        val move = pad()
        move.down(0, listOf(1000 to 1000))
        val moved = move.move(listOf((1000 + GestureMap.MOVE_THRESHOLD + 20) to 1000))
        assertTrue(moved.none { it is PointerSession.Out.Mouse && it.kind == "down" })
        assertTrue(moved.any { it is PointerSession.Out.Mouse && it.kind == "move" })
        assertTrue(move.up(200).none { it is PointerSession.Out.Mouse && it.kind == "down" })
        assertFalse(move.lastWasTap)
    }

    @Test
    fun movementPastSlopCancelsALongPress() {
        val session = pad()
        session.down(0, listOf(1000 to 1000))
        session.move(listOf((1000 + GestureMap.MOVE_THRESHOLD + 20) to 1000))
        assertTrue(session.poll(GestureMap.LONG_PRESS_MS + 20) == null)
        assertTrue(session.up(GestureMap.LONG_PRESS_MS + 40).none { it is PointerSession.Out.Mouse && it.kind == "down" })
    }

    @Test
    fun aStillHoldStartsADragAfterTheLongPress() {
        val session = pad()
        session.down(0, listOf(2000 to 2000))
        assertTrue(session.poll(GestureMap.LONG_PRESS_MS - 50) == null)
        val held = session.poll(GestureMap.LONG_PRESS_MS) as PointerSession.Out.Mouse
        assertEquals("down", held.kind)
        assertEquals("left", held.button)
        val drag = session.move(listOf((2000 + GestureMap.MOVE_THRESHOLD + 20) to 2000))
        assertTrue(drag.any { it is PointerSession.Out.Mouse && it.kind == "move" })
        assertTrue(drag.none { it is PointerSession.Out.Mouse && it.kind == "down" })
        assertEquals("up", (session.up(GestureMap.LONG_PRESS_MS + 80).single() as PointerSession.Out.Mouse).kind)
    }

    @Test
    fun twoFingerTapIsARightClick() {
        val session = pad()
        session.down(0, listOf(1000 to 1000, 1400 to 1100))
        val click = session.up(80).filterIsInstance<PointerSession.Out.Mouse>()
        assertEquals(listOf("down", "up"), click.map { it.kind })
        assertTrue(click.all { it.button == "right" })
    }

    @Test
    fun aSlowOrMovingTwoFingerStrokeIsNotARightClick() {
        val held = pad()
        held.down(0, listOf(1000 to 1000, 1400 to 1100))
        assertTrue(held.up(GestureMap.TWO_FINGER_TAP_MS + 20).isEmpty())

        val scrolled = pad()
        scrolled.down(0, listOf(100 to 1000, 120 to 1000))
        scrolled.move(listOf(100 to 1000 + GestureMap.SCROLL_SLOP + 40, 120 to 1000 + GestureMap.SCROLL_SLOP + 40))
        assertTrue(scrolled.up(80).none { it is PointerSession.Out.Mouse && it.button == "right" })
    }

    @Test
    fun twoFingerVerticalSlideScrollsAndFasterIsLarger() {
        val slow = pad()
        slow.scrollSpeed = 1f
        val slowWheel = wheelOf(slow, listOf(100 to 1000, 120 to 1000), listOf(100 to 3000, 120 to 3000))!!
        assertEquals("wheel", slowWheel.kind)
        assertTrue(slowWheel.dy < 0)

        val fast = pad()
        fast.scrollSpeed = 2f
        val fastWheel = wheelOf(fast, listOf(100 to 1000, 120 to 1000), listOf(100 to 3000, 120 to 3000))!!
        assertTrue(kotlin.math.abs(fastWheel.dy) > kotlin.math.abs(slowWheel.dy))

        val flipped = pad()
        flipped.scrollSpeed = 2f
        flipped.invertScroll = true
        val invertWheel = wheelOf(flipped, listOf(100 to 1000, 120 to 1000), listOf(100 to 3000, 120 to 3000))!!
        assertEquals(-fastWheel.dy, invertWheel.dy)
    }

    @Test
    fun twoFingerHorizontalSlideScrollsSideways() {
        val session = pad()
        val wheel = wheelOf(session, listOf(1000 to 100, 1000 to 120), listOf(3000 to 100, 3000 to 120))!!
        assertEquals("wheel", wheel.kind)
        assertTrue(wheel.dx < 0)
        assertEquals(0, wheel.dy)
    }

    @Test
    fun aPinchDoesNotRightClick() {
        // A large span change is still movement, so a lift is not a two-finger tap.
        val session = pad()
        session.down(0, listOf(1000 to 1000, 4000 to 4000))
        session.move(listOf(500 to 500, 8000 to 8000))
        assertTrue(session.up(80).none { it is PointerSession.Out.Mouse && it.button == "right" })
        session.cancel()
        assertTrue(session.up(100).isEmpty())
    }

    @Test
    fun aSmallTwoFingerMoveIsStillATap() {
        val session = pad()
        session.down(0, listOf(1000 to 1000, 1400 to 1100))
        session.move(listOf(1010 to 1010, 1410 to 1110))
        val click = session.up(60).filterIsInstance<PointerSession.Out.Mouse>()
        assertEquals(listOf("down", "up"), click.map { it.kind })
        assertTrue(click.all { it.button == "right" })
    }
}
