package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.core.ENGINE_LIBRARY_MISSING
import us.spader.pjdesk.core.Native
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.SavedFilter
import us.spader.pjdesk.core.SavedSort
import us.spader.pjdesk.core.applyLan
import us.spader.pjdesk.core.applyPresence
import us.spader.pjdesk.core.arrangeSaved
import us.spader.pjdesk.core.connectFailureText
import us.spader.pjdesk.core.liveOnline
import us.spader.pjdesk.core.osDetail
import us.spader.pjdesk.core.PresenceReport
import us.spader.pjdesk.core.presenceOnlineFlag
import us.spader.pjdesk.core.presenceStale
import us.spader.pjdesk.core.mergeSavedFromAccount
import us.spader.pjdesk.core.savedComputerCountLine
import us.spader.pjdesk.core.savedSyncChoiceOnUpdate
import us.spader.pjdesk.core.signInPauseSeconds
import us.spader.pjdesk.core.signInPauseText
import us.spader.pjdesk.core.redactConnectError
import us.spader.pjdesk.ui.ChatGate
import us.spader.pjdesk.ui.chatButtonOn
import us.spader.pjdesk.ui.chatGate
import us.spader.pjdesk.ui.showChatNeedsTip
import us.spader.pjdesk.core.savedSubtitle
import us.spader.pjdesk.core.shownSaved
import us.spader.pjdesk.core.shownVersion
import us.spader.pjdesk.core.versionBehind

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
    fun presenceReplacesAStaleOfflineDot() {
        val cached = listOf(
            SavedComputer(id = "902488771", name = "Cabin Desk", hostname = "CABINDESK", osFamily = "windows", osVersion = "11", online = false, lastSeen = 1_700_000_000),
            SavedComputer(id = "610978221", hostname = "CHIP20", osFamily = "windows", osVersion = "11", online = false, lastSeen = 1_700_000_000),
            SavedComputer(id = "622775091", hostname = "BEN-SURFACE-9", osFamily = "windows", osVersion = "11", online = true, lastSeen = 1_700_010_000),
        )
        val live = applyPresence(
            cached,
            listOf(
                PresenceReport(id = "902 488 771", hostname = "CABINDESK", lastSeen = 1_700_014_400, online = true),
                PresenceReport(id = "610978221", hostname = "CHIP20", lastSeen = 1_700_014_400_000, online = true),
                PresenceReport(id = "622775091", hostname = "BEN-SURFACE-9", lastSeen = 1_700_010_000, online = false),
            ),
        )
        assertEquals(true, live[0].online)
        assertEquals(1_700_014_400L, live[0].lastSeen)
        assertEquals(true, live[1].online)
        assertEquals(1_700_014_400L, live[1].lastSeen)
        assertEquals(false, live[2].online)
        assertEquals(1_700_010_000L, live[2].lastSeen)
        assertEquals(false, liveOnline(false, false))
        assertEquals(true, liveOnline(false, true))
        assertEquals(true, presenceOnlineFlag(true))
        assertEquals(true, presenceOnlineFlag(1))
        assertEquals(false, presenceOnlineFlag(0))
        val onLan = applyLan(live, setOf("622775091"))
        assertEquals(true, onLan[2].online)
        assertEquals(false, applyLan(listOf(cached[0]), emptySet())[0].online)
    }

    @Test
    fun aReplacedPhoneDropsAfterThreeDaysAndADayOfflineStays() {
        val now = 1_700_000_000_000L
        val day = SavedComputer(id = "610978221", hostname = "CHIP20", online = false, lastSeen = now / 1000 - 24 * 3600)
        val week = SavedComputer(id = "386626882", hostname = "Pixel 11 Pro XL", online = false, lastSeen = now / 1000 - 6 * 24 * 3600)
        val shown = arrangeSaved(
            listOf(day, week, cabin),
            recentIds = emptyList(),
            sort = SavedSort.Name,
            filter = SavedFilter.All,
            query = "",
            recentOnly = false,
            nowMs = now,
        )
        assertEquals(listOf("902488771", "610978221"), shown.map { it.id })
        assertFalse(presenceStale(day, now))
        assertTrue(presenceStale(week, now))
    }

    @Test
    fun subtitleIsIdOsAndAge() {
        val threeDaysMs = 3L * 24 * 60 * 60 * 1000
        val seen = (1_700_000_000_000L - threeDaysMs) / 1000
        val row = cabin.copy(online = false, lastSeen = seen)
        assertEquals("902 488 771 · Win 11 · 3d ago", savedSubtitle(row, nowMs = 1_700_000_000_000L))
        val stale = cabin.copy(appVersion = "0.6.43")
        val staleLine = savedSubtitle(stale, nowMs = 1_700_000_000_000L)
        assertFalse(staleLine.contains("now"))
        assertFalse(staleLine.contains("0.6.43"))
        val fresh = cabin.copy(presenceFresh = true, appVersion = "0.6.64")
        assertEquals("902 488 771 · Win 11 · now · 0.6.64", savedSubtitle(fresh, nowMs = 1_700_000_000_000L))
        assertEquals("902 488 771 · Win 11 · now · 0.6.43", savedSubtitle(row.copy(appVersion = "0.6.43"), nowMs = 1_700_000_000_000L, mine = true))
        assertEquals("0.6.46", shownVersion("0.6.43", mine = true, selfVersion = "0.6.46"))
        assertEquals("0.6.43", shownVersion("0.6.43", mine = false, selfVersion = "0.6.46"))
        assertEquals("0.6.43", shownVersion("0.6.43", mine = true, selfVersion = ""))
        assertTrue(versionBehind("0.6.43", "0.6.44"))
        assertFalse(versionBehind("0.6.44", ""))
    }

    @Test
    fun chatUsesTheHandshakeVersionThenFreshPresence() {
        assertEquals(ChatGate.Ready, chatGate("0.6.64", "0.6.43", true))
        assertEquals(ChatGate.TooOld, chatGate("0.6.49", "0.6.64", true))
        assertEquals(ChatGate.Ready, chatGate("", "0.6.64", true))
        assertEquals(ChatGate.TooOld, chatGate("", "0.6.43", true))
        assertEquals(ChatGate.Unknown, chatGate("", "0.6.64", false))
        assertEquals(ChatGate.Unknown, chatGate("", "", false))
        assertEquals(ChatGate.Ready, chatGate("11", "0.6.64", true))
        assertEquals(ChatGate.Unknown, chatGate("10.0.22631", "", false))
        assertTrue(chatButtonOn(chatReady = false, chatGate("0.6.64", "", false)))
        assertFalse(showChatNeedsTip(chatReady = false, chatGate("", "", false)))
        assertTrue(showChatNeedsTip(chatReady = false, chatGate("0.6.49", "", false)))
        assertFalse(showChatNeedsTip(chatReady = true, chatGate("0.6.49", "", false)))
    }

    @Test
    fun aStoredOnlineFlagIsNotShownUntilPresenceArrives() {
        val stored = listOf(cabin.copy(online = true, appVersion = "0.6.1"))
        val cleared = shownSaved(stored, emptyList())
        assertFalse(cleared[0].online)
        assertEquals("", cleared[0].appVersion)
        assertFalse(savedSubtitle(cleared[0], nowMs = 1_700_000_000_000L).contains("now"))
        val live = shownSaved(
            stored,
            listOf(PresenceReport(id = "902488771", online = true, appVersion = "0.6.64", lastSeen = 1_700_000_000)),
        )
        assertTrue(live[0].online)
        assertTrue(live[0].presenceFresh)
        assertEquals("0.6.64", live[0].appVersion)
        assertTrue(savedSubtitle(live[0], nowMs = 1_700_000_000_000L).contains("now"))
        assertTrue(savedSubtitle(live[0], nowMs = 1_700_000_000_000L).contains("0.6.64"))
        val hidden = redactConnectError("connect failed token=abcdef0123456789abcdef0123456789 password=secret")
        assertFalse(hidden.contains("secret"))
        assertFalse(hidden.contains("abcdef"))
    }

    @Test
    fun aLocalClearStaysUntilTheNextSignIn() {
        assertFalse(mergeSavedFromAccount(localCleared = true, preferAccount = false))
        assertTrue(mergeSavedFromAccount(localCleared = true, preferAccount = true))
        assertTrue(mergeSavedFromAccount(localCleared = false, preferAccount = false))
        assertEquals("This computer has 3 saved computers.", savedComputerCountLine(3))
        assertEquals("This computer has 1 saved computer.", savedComputerCountLine(1))
    }

    @Test
    fun anExistingListKeepsSyncingAndANewPhoneDoesNot() {
        assertEquals("yes", savedSyncChoiceOnUpdate("", 2))
        assertEquals("", savedSyncChoiceOnUpdate("", 0))
        assertEquals("no", savedSyncChoiceOnUpdate("no", 4))
        assertEquals("yes", savedSyncChoiceOnUpdate("yes", 0))
    }

    @Test
    fun aLogin429UsesBensSentenceAndAClock() {
        val production = "Too many failed sign-ins. Wait 15 minutes and try again."
        assertEquals(15L * 60, signInPauseSeconds(429, production))
        assertEquals(15L * 60, signInPauseSeconds(429, "PJDesk could not sign in (429)."))
        assertEquals(150L, signInPauseSeconds(429, "ignored", 150))
        assertEquals(61L, signInPauseSeconds(429, "Wait 15 minutes", 61))
        assertEquals(15L * 60, signInPauseSeconds(429, "ignored", 100_000))
        assertEquals(null, signInPauseSeconds(429, "Too many tries. Try again in 2 minutes.", 120))
        assertEquals(null, signInPauseSeconds(401, "Wrong email or password."))
        assertEquals(
            null,
            signInPauseSeconds(429, "PJDesk Cloud is temporarily unavailable. This computer keeps working on the local network."),
        )
        assertEquals("Too many sign-in attempts. Try again in 15 minutes.", signInPauseText(15 * 60))
        assertEquals("Too many sign-in attempts. Try again in 1 minute.", signInPauseText(1))
        assertEquals("Too many sign-in attempts. Try again in 2 minutes.", signInPauseText(61))
        assertEquals("Too many sign-in attempts. Try again in 3 minutes.", signInPauseText(150))
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
