package us.spader.pjdesk.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class ChatTipTest {
    @Test
    fun versionMessageOnlyForAKnownOldVersion() {
        assertEquals(CHAT_NEEDS_TIP, chatOffTip(chatGate("0.6.49", "", false)))
        assertEquals(CHAT_OFF_TIP, chatOffTip(chatGate("0.6.75", "", false)))
        assertEquals(CHAT_WAIT_TIP, chatOffTip(chatGate("", "", false)))
    }
}
