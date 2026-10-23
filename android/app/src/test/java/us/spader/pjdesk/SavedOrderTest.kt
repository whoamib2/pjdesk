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
    fun missingLibraryIsTheConnectError() {
        assertEquals(false, Native.loaded)
        assertEquals(ENGINE_LIBRARY_MISSING, Native.loadError)
        assertEquals(
            "Engine library missing from this build",
            connectFailureText(ENGINE_LIBRARY_MISSING, "Cabin Desk"),
        )
    }
}
