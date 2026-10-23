package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.a11y.ConsentWatch

class ConsentWatchTest {
    @Test
    fun aBoundUnlockedPhoneCanTap() {
        val decision = ConsentWatch.decide(bound = true, locked = false, polls = 1)
        assertTrue(decision.click)
        assertTrue(decision.again)
        assertFalse(decision.giveUp)
    }

    @Test
    fun aLockedPhoneIsNotTappedAndTheWatchKeepsGoing() {
        val decision = ConsentWatch.decide(bound = true, locked = true, polls = 4)
        assertFalse(decision.click)
        assertTrue(decision.again)
        assertFalse(decision.giveUp)
        assertTrue(decision.note!!.contains("cannot be tapped"))
    }

    @Test
    fun aMissingServiceKeepsWaiting() {
        val decision = ConsentWatch.decide(bound = false, locked = false, polls = 2)
        assertFalse(decision.click)
        assertTrue(decision.again)
        assertFalse(decision.giveUp)
        assertTrue(decision.note!!.contains("not bound"))
    }

    @Test
    fun theWatchGivesUpWhileThePhoneIsStillLocked() {
        val decision = ConsentWatch.decide(bound = true, locked = true, polls = ConsentWatch.POLL_LIMIT)
        assertFalse(decision.click)
        assertTrue(decision.giveUp)
        assertTrue(decision.note!!.contains("keyguard is locked"))
    }

    @Test
    fun theWatchGivesUpWhenNothingWasTapped() {
        val decision = ConsentWatch.decide(bound = true, locked = false, polls = ConsentWatch.POLL_LIMIT)
        assertEquals(true, decision.giveUp)
        assertTrue(decision.note!!.contains("could not click"))
    }
}
