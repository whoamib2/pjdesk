package us.spader.pjdesk

import android.accessibilityservice.AccessibilityService
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import us.spader.pjdesk.a11y.PjAccessibilityService

class NavActionTest {
    @Test
    fun backHomeAndRecentsAreGlobalActions() {
        assertEquals(AccessibilityService.GLOBAL_ACTION_BACK, PjAccessibilityService.navAction(3))
        assertEquals(AccessibilityService.GLOBAL_ACTION_HOME, PjAccessibilityService.navAction(4))
        assertEquals(AccessibilityService.GLOBAL_ACTION_RECENTS, PjAccessibilityService.navAction(5))
        assertNull(PjAccessibilityService.navAction(6))
        assertNull(PjAccessibilityService.navAction(0))
    }
}
