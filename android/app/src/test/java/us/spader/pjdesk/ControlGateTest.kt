package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import us.spader.pjdesk.core.ControlGate

class ControlGateTest {
    @Test
    fun theFirstReportIsImmediateAndARepeatIsQuiet() {
        val gate = ControlGate(holdOffMs = 2_000)
        assertEquals(false, gate.offer(false, 0))
        assertNull(gate.offer(false, 100))
        assertEquals(true, gate.offer(true, 200))
        assertNull(gate.offer(true, 300))
    }

    @Test
    fun aShortDropDoesNotPublishOff() {
        val gate = ControlGate(holdOffMs = 2_000)
        assertEquals(true, gate.offer(true, 0))
        assertNull(gate.offer(false, 1_000))
        assertNull(gate.offer(true, 1_500))
        assertNull(gate.offer(true, 3_000))
    }

    @Test
    fun aDropThatStaysDownIsPublishedAfterTheHold() {
        val gate = ControlGate(holdOffMs = 2_000)
        assertEquals(true, gate.offer(true, 0))
        assertNull(gate.offer(false, 100))
        assertEquals(false, gate.offer(false, 2_100))
        assertNull(gate.offer(false, 2_200))
    }
}
