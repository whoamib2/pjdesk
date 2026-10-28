package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.core.accountStatusLine
import us.spader.pjdesk.core.blockOfflineConnect
import us.spader.pjdesk.core.connectFailureText
import us.spader.pjdesk.core.connectStatusLabel
import us.spader.pjdesk.core.sessionAfterRefreshFailure
import us.spader.pjdesk.core.shouldRetryNotOnline
import us.spader.pjdesk.core.signInPageError

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
            "Cabin Desk did not answer through the PJDesk server. This phone reached the server, but that computer did not start a session.",
            connectFailureText("the host did not respond", "Cabin Desk"),
        )
        assertEquals(
            "BENLENOVODESK did not answer through the PJDesk server. It is listed as online, so this phone could not start a session on the server it reached.",
            connectFailureText("the PJDesk server did not start a session", "BENLENOVODESK", listedOnline = true),
        )
        assertEquals(
            "BENLENOVODESK is on the PJDesk server, but it did not finish the relay setup.",
            connectFailureText("the computer did not finish the relay setup", "BENLENOVODESK"),
        )
        assertEquals("Your sign-in expired", connectFailureText("Sign in required", "Cabin Desk"))
        assertEquals(
            "Could not reach the PJDesk server at 204.248.122.44:21200 (connection refused). This phone is not on that server's local network, so BENLENOVODESK could not be connected.",
            connectFailureText(
                "Could not reach the PJDesk server at 204.248.122.44:21200 (connection refused). This phone is not on that server's local network, so 902 488 771 could not be connected.",
                "BENLENOVODESK",
            ),
        )
        assertEquals(
            "no UDP reply from the server at 204.248.122.44:21201. Make sure UDP port 21201 is allowed",
            connectFailureText(
                "no UDP reply from the server at 204.248.122.44:21201. Make sure UDP port 21201 is allowed",
                "BENLENOVODESK",
            ),
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
    fun accountStatusNamesTheSession() {
        assertEquals("Not signed in", accountStatusLine("", "valid", false))
        assertEquals("Signed in as ben@spader.us. Session valid.", accountStatusLine("ben@spader.us", "valid", false))
        assertEquals("Signed in as ben@spader.us. Session expired.", accountStatusLine("ben@spader.us", "expired", false))
        assertEquals("Signed in as ben@spader.us. Session not checked.", accountStatusLine("ben@spader.us", "", false))
        assertEquals(
            "Signed in as ben@spader.us until you close PJDesk. Session valid.",
            accountStatusLine("ben@spader.us", "expired", true),
        )
        assertEquals("expired", sessionAfterRefreshFailure("Sign in required"))
        assertEquals("expired", sessionAfterRefreshFailure("Wrong email or password"))
        assertNull(sessionAfterRefreshFailure("Waiting before asking pjdesk.com again."))
    }

    @Test
    fun aComputerSeenJustNowIsNotBlockedAsOffline() {
        val now = 1_700_000_000_000L
        assertFalse(blockOfflineConnect(online = true, lastSeen = now / 1000 - 30, nowMs = now))
        assertFalse(blockOfflineConnect(online = false, lastSeen = now / 1000 - 30, nowMs = now))
        assertTrue(blockOfflineConnect(online = false, lastSeen = now / 1000 - 120, nowMs = now))
        assertTrue(blockOfflineConnect(online = false, lastSeen = 0, nowMs = now))
    }

    @Test
    fun theSignInPageDoesNotShowTheRawRefusal() {
        assertEquals("Wrong email or password.", signInPageError("Wrong email or password."))
        assertEquals(
            "Too many sign-in attempts. Try again in 3 minutes.",
            signInPageError("Too many sign-in attempts. Try again in 3 minutes.", 150),
        )
        assertEquals("Could not reach the PJDesk server.", signInPageError("the server did not answer"))
        assertEquals("Could not reach the PJDesk server.", signInPageError(""))
        assertEquals(
            "This server is too old to accept a sign-in while Require signed-in accounts is on. Update the server.",
            signInPageError("Sign in required."),
        )
        assertEquals(
            "This server is too old to accept a sign-in while Require signed-in accounts is on. Update the server.",
            signInPageError("The server at 192.168.0.193 requires sign-in. Update it to 0.6.52 or turn off Require signed-in accounts."),
        )
    }

    @Test
    fun connectingNamesTheComputer() {
        assertEquals("Connecting to Cabin Desk…", connectStatusLabel("Cabin Desk", "902488771"))
        assertEquals("Connecting to 902 488 771…", connectStatusLabel("", "902488771"))
    }
}
