package us.spader.pjdesk

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.service.EntireDisplay

class EntireDisplayTest {
    @Test
    fun thePhysicalDisplayInEitherOrientationIsTheEntireScreen() {
        assertFalse(EntireDisplay.isSingleApp(1080, 2400, 1080, 2400))
        assertFalse(EntireDisplay.isSingleApp(2400, 1080, 1080, 2400))
    }

    @Test
    fun aFewPixelsOfRoundingIsStillTheEntireScreen() {
        assertFalse(EntireDisplay.isSingleApp(1080, 2390, 1080, 2400))
    }

    @Test
    fun aShorterAppWindowIsASingleApp() {
        assertTrue(EntireDisplay.isSingleApp(1080, 2264, 1080, 2400))
        assertTrue(EntireDisplay.isSingleApp(540, 1200, 1080, 2400))
    }

    @Test
    fun anUnknownSizeIsNotTreatedAsASingleApp() {
        assertFalse(EntireDisplay.isSingleApp(0, 0, 1080, 2400))
        assertFalse(EntireDisplay.isSingleApp(1080, 2400, 0, 0))
    }
}
