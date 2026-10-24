package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.core.ControlPad
import us.spader.pjdesk.core.RemoteOptions
import us.spader.pjdesk.core.SessionHit

class SessionHitTest {
    @Test
    fun aTapBelowTheSlopOpensAndTheOldDragRuleDoesNot() {
        val pixelSlop = 24f
        assertTrue(SessionHit.tapOpens(movedPx = 4f, slopPx = pixelSlop))
        assertFalse(SessionHit.tapOpens(movedPx = 30f, slopPx = pixelSlop))
        assertFalse(SessionHit.legacyDragOpens(movedPx = 4f, slopPx = pixelSlop))
        assertFalse(SessionHit.legacyDragOpens(movedPx = 30f, slopPx = pixelSlop))
        assertTrue(SessionHit.legacyDragOpens(movedPx = 10f, slopPx = 8f))
    }

    @Test
    fun theGestureLayerSkipsATouchInsideAControl() {
        val gear = ControlPad(400f, 20f, 456f, 76f)
        assertTrue(SessionHit.covered(420f, 40f, listOf(gear)))
        assertFalse(SessionHit.covered(10f, 10f, listOf(gear)))
        assertFalse(SessionHit.covered(456f, 40f, listOf(gear)))
    }

    @Test
    fun qualityNamesAndPermissionBits() {
        assertEquals("high", RemoteOptions.preset("best"))
        assertEquals("smooth", RemoteOptions.preset("speed"))
        assertEquals("balanced", RemoteOptions.preset("balanced"))
        assertTrue(RemoteOptions.allows(-1, RemoteOptions.PERM_PRIVACY))
        assertFalse(RemoteOptions.allows(0, RemoteOptions.PERM_PRIVACY))
        assertTrue(RemoteOptions.allows(RemoteOptions.PERM_FILE, RemoteOptions.PERM_FILE))
        assertEquals(60, RemoteOptions(customFps = 90).cleaned().customFps)
    }
}
