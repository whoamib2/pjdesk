package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Test
import us.spader.pjdesk.ui.lockVersionLabel

class LockVersionTest {
    @Test fun showsVersion() = assertEquals("PJDesk 0.6.87", lockVersionLabel("0.6.87"))
    @Test fun missingVersionFallsBack() = assertEquals("PJDesk", lockVersionLabel(null))
}
