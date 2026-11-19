package us.spader.pjdesk.input

import android.view.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class HardwareKeysTest {
    @Test
    fun lettersAndModifiersMapToSetOneScanCodes() {
        assertEquals(0x1E, HardwareKeys.scan(KeyEvent.KEYCODE_A))
        assertEquals(0x0F, HardwareKeys.scan(KeyEvent.KEYCODE_TAB))
        assertEquals(0x01, HardwareKeys.scan(KeyEvent.KEYCODE_ESCAPE))
        assertEquals(0x1D, HardwareKeys.scan(KeyEvent.KEYCODE_CTRL_LEFT))
        assertEquals(0xE05B, HardwareKeys.scan(KeyEvent.KEYCODE_META_LEFT))
        assertEquals(0xE05C, HardwareKeys.scan(KeyEvent.KEYCODE_META_RIGHT))
        assertEquals(0xE05D, HardwareKeys.scan(KeyEvent.KEYCODE_MENU))
        assertEquals(0xE04B, HardwareKeys.scan(KeyEvent.KEYCODE_DPAD_LEFT))
        assertEquals(0xE037, HardwareKeys.scan(KeyEvent.KEYCODE_SYSRQ))
    }

    @Test
    fun systemButtonsStayOnThePhone() {
        assertFalse(HardwareKeys.captures(KeyEvent.KEYCODE_BACK, KeyEvent.ACTION_DOWN))
        assertFalse(HardwareKeys.captures(KeyEvent.KEYCODE_VOLUME_UP, KeyEvent.ACTION_DOWN))
        assertNull(HardwareKeys.scan(KeyEvent.KEYCODE_BACK))
    }

    @Test
    fun escapeAndTabAreCaptured() {
        assertTrue(HardwareKeys.captures(KeyEvent.KEYCODE_ESCAPE, KeyEvent.ACTION_DOWN))
        assertTrue(HardwareKeys.captures(KeyEvent.KEYCODE_TAB, KeyEvent.ACTION_UP))
        assertTrue(HardwareKeys.captures(KeyEvent.KEYCODE_META_LEFT, KeyEvent.ACTION_DOWN))
    }
}
