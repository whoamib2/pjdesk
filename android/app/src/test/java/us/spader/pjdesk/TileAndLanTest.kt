package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import us.spader.pjdesk.core.LanSight
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.SavedView
import us.spader.pjdesk.core.acceptLanHost
import us.spader.pjdesk.core.decodeLanBeacon
import us.spader.pjdesk.core.discoveredComputers
import us.spader.pjdesk.core.tileCaption
import us.spader.pjdesk.core.tileIndex
import us.spader.pjdesk.core.tileKey
import us.spader.pjdesk.core.tileRgb

class TileAndLanTest {
    @Test
    fun tileColorMatchesTheDesktopHash() {
        assertEquals(2, tileIndex("692996394"))
        assertEquals(0x5B3A78, tileRgb("692996394"))
        assertEquals(tileRgb("692996394"), tileRgb(tileKey("692 996 394", "ignored")))
        assertEquals(7, tileIndex("902488771"))
        assertEquals(0x3D454D, tileRgb("902488771"))
        assertEquals(8, tileIndex("622775091"))
        assertEquals(0x8A5A1E, tileRgb("622775091"))
        assertEquals(0, tileIndex("482636105"))
        assertEquals(0x5C3A2E, tileRgb("482636105"))
        assertEquals(tileIndex("front desk"), tileIndex(tileKey("", "Front Desk")))
        assertEquals(tileRgb("692996394"), tileRgb("692996394"))
    }

    @Test
    fun viewDefaultsToTheList() {
        assertEquals(SavedView.List, SavedView.fromStorage(null))
        assertEquals(SavedView.List, SavedView.fromStorage("nope"))
        assertEquals(SavedView.Small, SavedView.fromStorage("small"))
        assertEquals(SavedView.Big, SavedView.fromStorage("big"))
        assertEquals("list", SavedView.List.storage)
    }

    @Test
    fun beaconMatchesTheDesktopPacket() {
        val packet = hex("504a4445534b4c4200380309313233343536373839d1a50109636162696e6465736b" + "07".repeat(32))
        val body = decodeLanBeacon(packet)
        assertEquals("123456789", body!!.id)
        assertEquals(21201, body.port)
        assertEquals("cabindesk", body.name)
        assertNull(decodeLanBeacon("PJDESKSV".toByteArray()))
        assertNull(decodeLanBeacon(packet.copyOf(9)))
    }

    @Test
    fun lanHostsArePrivateOnly() {
        assertEquals(true, acceptLanHost("192.168.1.20"))
        assertEquals(true, acceptLanHost("10.1.2.3"))
        assertEquals(true, acceptLanHost("100.64.1.1"))
        assertEquals(false, acceptLanHost("8.8.8.8"))
        assertEquals(false, acceptLanHost("0.0.0.0"))
        assertEquals(true, acceptLanHost("fe80::1"))
    }

    @Test
    fun discoveredKeepsTheSavedPlatformAndSkipsThisPhone() {
        val saved = SavedComputer(id = "902488771", name = "Cabin Desk", osFamily = "windows", osVersion = "11", hostname = "CABINDESK", online = false)
        val sights = listOf(
            LanSight("902488771", "CABINDESK", "192.168.1.20", 21201),
            LanSight("482636105", "Ben Pixel 11", "192.168.1.8", 21201),
            LanSight("111555694", "Office", "10.0.0.4", 21201),
            LanSight("902488771", "CABINDESK", "192.168.1.20", 21201),
        )
        val rows = discoveredComputers(sights, listOf(saved), selfId = "482 636 105")
        assertEquals(listOf("902488771", "111555694"), rows.map { it.id })
        assertEquals("windows", rows[0].osFamily)
        assertEquals(true, rows[0].online)
        assertEquals("Office", rows[1].hostname)
        assertEquals("Ben Spader@CABINDESK", tileCaption(saved.copy(consoleUser = "Ben Spader")))
    }

    private fun hex(text: String): ByteArray {
        val out = ByteArray(text.length / 2)
        for (i in out.indices) {
            out[i] = text.substring(i * 2, i * 2 + 2).toInt(16).toByte()
        }
        return out
    }
}
