package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Test
import us.spader.pjdesk.ui.connectPasswordMessage

class ConnectPasswordMessageTest {
    @Test
    fun theMessageIsOneShortLine() {
        assertEquals("Saved password didn't work. Enter it again.", connectPasswordMessage("", true, true))
        assertEquals("That password didn't work. Try again.", connectPasswordMessage("", true, false))
        assertEquals("Enter the password for this computer.", connectPasswordMessage("", false, false))
        assertEquals("Locked for 5 minutes", connectPasswordMessage("Locked for 5 minutes\nmore", true, true))
    }
}
