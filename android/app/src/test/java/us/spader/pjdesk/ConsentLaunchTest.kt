package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.a11y.PjAccessibilityService
import us.spader.pjdesk.service.ConsentLaunch
import us.spader.pjdesk.service.ControlNotice

class ConsentLaunchTest {
    @Test
    fun fullScreenIntentIsGrantedUntilAndroid14Asks() {
        assertTrue(ConsentLaunch.fullScreenAllowed(33, false))
        assertFalse(ConsentLaunch.fullScreenAllowed(34, false))
        assertTrue(ConsentLaunch.fullScreenAllowed(34, true))
    }

    @Test
    fun theControlNoticeNamesTheDeepLink() {
        assertEquals("Control is off, tap to turn on", ControlNotice.TEXT)
    }

    @Test
    fun accessibilitySettingsOpenTheServicePage() {
        assertEquals(
            "android.settings.ACCESSIBILITY_DETAILS_SETTINGS",
            PjAccessibilityService.accessibilitySettingsAction(33),
        )
        assertEquals(
            android.provider.Settings.ACTION_ACCESSIBILITY_SETTINGS,
            PjAccessibilityService.accessibilitySettingsAction(32),
        )
    }
}
