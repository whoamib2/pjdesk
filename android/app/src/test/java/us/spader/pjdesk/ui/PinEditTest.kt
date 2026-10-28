package us.spader.pjdesk.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class PinEditTest {
    @Test
    fun backspaceRemovesTheLastDigitTypedAndDeleteOrEscClears() {
        assertEquals("123", applyPinEdit("1234", PinEdit.Back))
        assertEquals("", applyPinEdit("", PinEdit.Back))
        assertEquals("", applyPinEdit("1234", PinEdit.Clear))
    }

    @Test
    fun dotsGrowFromTheMiddleWithNoSlots() {
        assertEquals(0, pinMarkOffsets(0, 18f).size)
        val five = pinMarkOffsets(5, 18f)
        assertTrue(kotlin.math.abs(five[0] + five[4]) < 0.01f)
    }
}
