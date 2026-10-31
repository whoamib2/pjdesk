package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Test
import us.spader.pjdesk.core.StartScreen

class StartScreenTest {
    @Test fun opensToSavedByDefault() {
        assertEquals(StartScreen.SAVED, StartScreen.normalize(null))
        assertEquals(1, StartScreen.tab(null))
        assertEquals(1, StartScreen.tab("garbage"))
    }

    @Test fun homeChoiceOpensHome() {
        assertEquals(0, StartScreen.tab(StartScreen.HOME))
        assertEquals(1, StartScreen.tab(StartScreen.SAVED))
    }
}
