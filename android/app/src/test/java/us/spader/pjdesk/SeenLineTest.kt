package us.spader.pjdesk

import java.util.TimeZone
import org.junit.Assert.assertEquals
import org.junit.Test
import us.spader.pjdesk.core.exactSeen
import us.spader.pjdesk.core.seenLine

class SeenLineTest {
    private val chicago = TimeZone.getTimeZone("America/Chicago")

    @Test
    fun exactLocalTimeWithZone() {
        // 2026-10-31 14:58:12 UTC is 9:58:12 AM CDT.
        val t = 1_793_458_692L
        assertEquals("Oct 31, 2026 9:58:12 AM CDT", exactSeen(t, chicago))
        assertEquals("Oct 31, 2026 9:58:12 AM CDT", exactSeen(t * 1000, chicago))
        assertEquals("Last seen Oct 31, 2026 9:58:12 AM CDT", seenLine(false, t, zone = chicago))
        assertEquals("Online · last report Oct 31, 2026 9:58:12 AM CDT", seenLine(true, t, zone = chicago))
        assertEquals("Last seen: never reported", seenLine(false, 0))
    }
}
