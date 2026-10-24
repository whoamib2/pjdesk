package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Test
import us.spader.pjdesk.core.ENGINE_LIBRARY_MISSING
import us.spader.pjdesk.core.Native
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.SavedFilter
import us.spader.pjdesk.core.SavedSort
import us.spader.pjdesk.core.arrangeSaved
import us.spader.pjdesk.core.connectFailureText
import us.spader.pjdesk.core.osDetail
import us.spader.pjdesk.core.savedSubtitle

class SavedOrderTest {
    private val cabin = SavedComputer(
        id = "902488771",
        name = "Cabin Desk",
        osFamily = "windows",
        osVersion = "11 26H2",
        hostname = "CABINDESK",
        consoleUser = "Ben Spader",
        online = true,
        lastSeen = 10,
    )
    private val surface = SavedComputer(
        id = "622775091",
        name = "BEN-SURFACE-9",
        osFamily = "windows",
        osVersion = "11 26H2",
        hostname = "BEN-SURFACE-9",
        online = false,
        lastSeen = 9,
    )
    private val lenovo = SavedComputer(
        id = "692996394",
        name = "BENLENOVODESK",
        osFamily = "windows",
        osVersion = "11",
        hostname = "BENLENOVODESK",
        online = true,
        lastSeen = 8,
    )

    @Test
    fun osDetailStaysOneLine() {
        assertEquals("Windows · 11 26H2", osDetail(cabin))
    }

    @Test
    fun onlineFirstThenName() {
        val shown = arrangeSaved(
            listOf(surface, cabin, lenovo),
            recentIds = listOf("622775091"),
            sort = SavedSort.Online,
            filter = SavedFilter.All,
            query = "",
            recentOnly = false,
        )
        assertEquals(listOf("692996394", "902488771", "622775091"), shown.map { it.id })
    }

    @Test
    fun nameSortAndOfflineFilter() {
        val named = arrangeSaved(
            listOf(surface, cabin, lenovo),
            recentIds = emptyList(),
            sort = SavedSort.Name,
            filter = SavedFilter.All,
            query = "",
            recentOnly = false,
        )
        assertEquals(listOf("622775091", "692996394", "902488771"), named.map { it.id })
        val offline = arrangeSaved(
            listOf(surface, cabin, lenovo),
            recentIds = emptyList(),
            sort = SavedSort.Name,
            filter = SavedFilter.Offline,
            query = "windows",
            recentOnly = false,
        )
        assertEquals(listOf("622775091"), offline.map { it.id })
    }

    @Test
    fun recentlyUsedKeepsUntouchedAtTheEnd() {
        val shown = arrangeSaved(
            listOf(surface, cabin, lenovo),
            recentIds = listOf("622775091", "902488771"),
            sort = SavedSort.Recent,
            filter = SavedFilter.All,
            query = "",
            recentOnly = false,
        )
        assertEquals(listOf("622775091", "902488771", "692996394"), shown.map { it.id })
    }

    @Test
    fun recentIsTheDefaultAndThisPhoneSinks() {
        assertEquals(SavedSort.Recent, SavedSort.fromStorage(null))
        assertEquals(SavedSort.Online, SavedSort.fromStorage("online"))
        val phone = SavedComputer(id = "482636105", hostname = "Ben Pixel 11", osFamily = "android", osVersion = "17", online = true)
        val shown = arrangeSaved(
            listOf(phone, surface, cabin),
            recentIds = listOf("482636105", "902488771"),
            sort = SavedSort.Recent,
            filter = SavedFilter.All,
            query = "",
            recentOnly = false,
            selfId = "482 636 105",
        )
        assertEquals(listOf("902488771", "622775091", "482636105"), shown.map { it.id })
    }

    @Test
    fun subtitleIsIdOsAndAge() {
        val threeDaysMs = 3L * 24 * 60 * 60 * 1000
        val seen = (1_700_000_000_000L - threeDaysMs) / 1000
        val row = cabin.copy(online = false, lastSeen = seen)
        assertEquals("902 488 771 · Win 11 · 3d ago", savedSubtitle(row, nowMs = 1_700_000_000_000L))
        assertEquals("902 488 771 · Win 11 · now", savedSubtitle(cabin, nowMs = 1_700_000_000_000L))
    }

    @Test
    fun missingLibraryIsTheConnectError() {
        assertEquals(false, Native.loaded)
        assertEquals(ENGINE_LIBRARY_MISSING, Native.loadError)
        assertEquals(
            "Engine library missing from this build",
            connectFailureText(ENGINE_LIBRARY_MISSING, "Cabin Desk"),
        )
    }
}
