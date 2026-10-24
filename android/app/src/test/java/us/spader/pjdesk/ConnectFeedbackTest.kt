package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.core.connectFailureText
import us.spader.pjdesk.core.connectStatusLabel
import us.spader.pjdesk.core.shouldRetryNotOnline

class ConnectFeedbackTest {
    @Test
    fun aReplacedOrCancelledAttemptIsQuiet() {
        assertNull(connectFailureText("stopped", "Cabin Desk"))
        assertNull(connectFailureText("cancelled", "Cabin Desk"))
        assertNull(connectFailureText("  ", "Cabin Desk"))
    }

    @Test
    fun relayOfflineLockoutAndMissingServerAreNamed() {
        assertEquals(
            "Could not reach Cabin Desk, even through the relay.",
            connectFailureText("could not reach the host, even through the relay", "Cabin Desk"),
        )
        assertEquals(
            "Cabin Desk is offline, or it did not answer.",
            connectFailureText("the host did not respond", "Cabin Desk"),
        )
        assertEquals(
            "BEN-SURFACE-9 is offline.",
            connectFailureText("BEN-SURFACE-9 is offline.", "BEN-SURFACE-9"),
        )
        assertEquals(
            "locked out, try again in 20 minutes",
            connectFailureText("locked out, try again in 20 minutes", "Cabin Desk"),
        )
        assertEquals(
            "This phone has no server address yet. Sign in again, or set the server in Settings.",
            connectFailureText("the server address is empty", "Cabin Desk"),
        )
        assertEquals(
            "Could not reach the PJDesk server, so Cabin Desk could not be connected.",
            connectFailureText("timed out connecting to the server at pjdesk.spader.us:21200", "Cabin Desk"),
        )
        assertEquals("Incorrect password for 902 488 771", connectFailureText("Incorrect password for 902 488 771", "Cabin Desk"))
    }

    @Test
    fun notOnlineRetriesWhenTheComputerWasSeenRecently() {
        val now = 1_700_000_000_000L
        val recent = now / 1000 - 30
        assertTrue(shouldRetryNotOnline("device 902 488 771 is not online", recent, now, 0))
        assertTrue(shouldRetryNotOnline("device 902 488 771 is not online", recent, now, 1))
        assertFalse(shouldRetryNotOnline("device 902 488 771 is not online", recent, now, 2))
        assertTrue(shouldRetryNotOnline("device 902 488 771 is not online", now - 30_000, now, 0))
        assertFalse(shouldRetryNotOnline("device 902 488 771 is not online", now / 1000 - 121, now, 0))
        assertFalse(shouldRetryNotOnline("device 902 488 771 is not online", 0, now, 0))
        assertFalse(shouldRetryNotOnline("Incorrect password for 902 488 771", recent, now, 0))
        assertFalse(shouldRetryNotOnline("Cabin Desk is offline.", recent, now, 0))
    }

    @Test
    fun connectingNamesTheComputer() {
        assertEquals("Connecting to Cabin Desk…", connectStatusLabel("Cabin Desk", "902488771"))
        assertEquals("Connecting to 902 488 771…", connectStatusLabel("", "902488771"))
    }
}
