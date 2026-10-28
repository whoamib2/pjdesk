package us.spader.pjdesk.core

enum class SavedSort(val storage: String) {
    Online("online"),
    Name("name"),
    Recent("recent"),
    ;

    companion object {
        fun fromStorage(value: String?): SavedSort = entries.firstOrNull { it.storage == value } ?: Recent
    }
}

enum class SavedFilter(val storage: String) {
    All("all"),
    Online("online"),
    Offline("offline"),
    ;

    companion object {
        fun fromStorage(value: String?): SavedFilter = entries.firstOrNull { it.storage == value } ?: All
    }
}

/** Big tiles, small tiles, or the compact list. List is the default. */
enum class SavedView(val storage: String) {
    List("list"),
    Small("small"),
    Big("big"),
    ;

    companion object {
        fun fromStorage(value: String?): SavedView = entries.firstOrNull { it.storage == value } ?: List
    }
}

fun computerTitle(computer: SavedComputer): String {
    val nick = computer.nickname()
    val host = computer.hostname.trim()
    val display = computer.displayName.trim().let { if (it.equals(host, ignoreCase = true)) "" else it }
    return nick.ifBlank { display.ifBlank { host.ifBlank { "No name yet" } } }
}

fun formatSavedId(id: String): String = id.filter { it.isDigit() }.chunked(3).joinToString(" ")

fun osDetail(computer: SavedComputer): String {
    val system = when (computer.osFamily.trim().lowercase()) {
        "windows" -> "Windows"
        "macos" -> "macOS"
        "linux" -> "Linux"
        "android" -> "Android"
        "" -> ""
        else -> computer.osFamily.trim().replaceFirstChar { it.uppercase() }
    }
    val version = computer.osVersion.trim()
    return listOf(system, version).filter { it.isNotEmpty() }.joinToString(" · ")
}

/** Short system name for a compact row, such as "Win 11". */
fun osShort(computer: SavedComputer): String {
    val version = computer.osVersion.trim().substringBefore(' ').trim()
    return when (computer.osFamily.trim().lowercase()) {
        "windows" -> if (version.isEmpty()) "Win" else "Win $version"
        "android" -> if (version.isEmpty()) "Android" else "Android $version"
        "macos" -> if (version.isEmpty()) "macOS" else "macOS $version"
        "linux" -> if (version.isEmpty()) "Linux" else "Linux $version"
        "" -> ""
        else -> osDetail(computer)
    }
}

/**
 * Unix seconds. The account stores seconds. A millisecond stamp is converted
 * so a later second-based report can still win the comparison.
 */
fun seenSeconds(stamp: Long): Long {
    if (stamp <= 0L) return 0L
    return if (stamp >= 10_000_000_000L) stamp / 1000 else stamp
}

/** Offline computers older than this are left out of the saved list. */
const val PRESENCE_OFFLINE_KEEP_SECS = 3 * 24 * 3600

/** A replaced phone from several days ago. One day offline still shows. A missing time does not. */
fun presenceStale(computer: SavedComputer, nowMs: Long): Boolean {
    if (computer.online) return false
    val seen = seenSeconds(computer.lastSeen)
    if (seen < 1_600_000_000L) return false
    return nowMs / 1000 - seen > PRESENCE_OFFLINE_KEEP_SECS
}

/** The account's online flag, or a 1/0 that some JSON readers leave as a number. */
fun presenceOnlineFlag(value: Any?): Boolean = when (value) {
    is Boolean -> value
    is Number -> value.toLong() != 0L
    is String -> value.equals("true", ignoreCase = true) || value == "1"
    else -> false
}

/**
 * Same rule as the Windows Saved list. Online when the account's live presence
 * says so, or when this phone just heard the computer on the local network.
 * The last time this phone connected is not presence.
 */
fun liveOnline(reportedOnline: Boolean, seenOnLan: Boolean): Boolean = reportedOnline || seenOnLan

/** This phone's own row shows the running version. Other rows show the stored one. */
fun shownVersion(reported: String, mine: Boolean, selfVersion: String): String {
    if (mine && selfVersion.isNotBlank()) return selfVersion
    return reported
}

/** Rows from an account or fleet `presence` array. A missing array is not fresh presence. */
fun presenceReports(seen: org.json.JSONArray?): List<PresenceReport> {
    if (seen == null) return emptyList()
    val out = ArrayList<PresenceReport>(seen.length())
    for (i in 0 until seen.length()) {
        val o = seen.optJSONObject(i) ?: continue
        val id = o.optString("id").filter { it.isDigit() }
        if (id.length != 9) continue
        val online = if (o.has("online") && !o.isNull("online")) o.get("online") else null
        out.add(
            PresenceReport(
                id = id,
                hostname = o.optString("hostname"),
                osFamily = o.optString("os_family").ifBlank { o.optString("osFamily") },
                osVersion = o.optString("os_version").ifBlank { o.optString("osVersion") },
                consoleUser = o.optString("console_user").ifBlank { o.optString("consoleUser") },
                displayName = o.optString("display_name").ifBlank { o.optString("displayName") },
                lastSeen = when {
                    o.has("last_seen") -> o.optLong("last_seen")
                    else -> o.optLong("lastSeen")
                },
                online = presenceOnlineFlag(online),
                appVersion = o.optString("app_version").ifBlank { o.optString("version") }.ifBlank { o.optString("appVersion") },
            ),
        )
    }
    return out
}

/** One row from the account presence list. `lastSeen` is unix seconds or milliseconds. */
data class PresenceReport(
    val id: String,
    val hostname: String = "",
    val osFamily: String = "",
    val osVersion: String = "",
    val consoleUser: String = "",
    val displayName: String = "",
    val lastSeen: Long = 0,
    val online: Boolean = false,
    val appVersion: String = "",
)

/**
 * Applies a presence pull. The online flag always comes from this report.
 * A newer last-seen time replaces an older one. A missing computer is left as it was.
 */
fun applyPresence(computers: List<SavedComputer>, presence: List<PresenceReport>): List<SavedComputer> {
    if (presence.isEmpty()) return computers
    val merged = computers.toMutableList()
    for (row in presence) {
        val id = row.id.filter { it.isDigit() }
        if (id.length != 9) continue
        val seen = seenSeconds(row.lastSeen)
        val index = merged.indexOfFirst { it.id.filter { ch -> ch.isDigit() } == id }
        if (index < 0) {
            merged.add(
                SavedComputer(
                    id = id,
                    hostname = row.hostname,
                    osFamily = row.osFamily,
                    osVersion = row.osVersion,
                    consoleUser = row.consoleUser,
                    displayName = row.displayName,
                    lastSeen = seen,
                    online = row.online,
                    appVersion = row.appVersion,
                    presenceFresh = true,
                ),
            )
            continue
        }
        val cur = merged[index]
        merged[index] = cur.copy(
            hostname = row.hostname.ifBlank { cur.hostname },
            osFamily = row.osFamily.ifBlank { cur.osFamily },
            osVersion = row.osVersion.ifBlank { cur.osVersion },
            consoleUser = row.consoleUser.ifBlank { cur.consoleUser },
            displayName = row.displayName.ifBlank { cur.displayName },
            lastSeen = maxOf(seenSeconds(cur.lastSeen), seen),
            online = row.online,
            appVersion = row.appVersion.ifBlank { cur.appVersion },
            presenceFresh = true,
        )
    }
    return merged
}

/**
 * Saved rows for the list on screen. Online and version come only from `presence`.
 * An empty report clears both, including a flag that was stored earlier.
 */
fun shownSaved(stored: List<SavedComputer>, presence: List<PresenceReport>): List<SavedComputer> {
    val base = stored.map { it.copy(online = false, appVersion = "", presenceFresh = false) }
    if (presence.isEmpty()) return base
    return applyPresence(base, presence)
}

/** Marks computers heard on the LAN online, without writing that into the saved file. */
fun applyLan(computers: List<SavedComputer>, lanIds: Set<String>): List<SavedComputer> {
    if (lanIds.isEmpty()) return computers
    return computers.map { computer ->
        val id = computer.id.filter { it.isDigit() }
        if (liveOnline(computer.online, id in lanIds) && !computer.online) {
            computer.copy(online = true, presenceFresh = true)
        } else {
            computer
        }
    }
}

/** "3d ago" from a unix-second or unix-millis stamp. Empty when the time is unknown. */
fun agoShort(lastSeen: Long, nowMs: Long): String {
    if (lastSeen <= 0L) return ""
    val seenMs = seenSeconds(lastSeen) * 1000
    val mins = ((nowMs - seenMs).coerceAtLeast(0L)) / 60_000L
    return when {
        mins < 1 -> "now"
        mins < 60 -> "${mins}m ago"
        mins < 60 * 24 -> "${mins / 60}h ago"
        else -> "${mins / (60 * 24)}d ago"
    }
}

/** A version column. Unparseable and empty reports are unknown. */
fun shownAppVersion(raw: String): String {
    val core = raw.trim().removePrefix("v").substringBefore('-').substringBefore('+')
    val parts = core.split('.')
    if (parts.size < 2 || parts.any { it.toIntOrNull() == null }) return "—"
    return core
}

/** True when both versions parse and the installed one is older. */
fun versionBehind(installed: String, latest: String): Boolean {
    fun parse(raw: String): IntArray? {
        val shown = shownAppVersion(raw)
        if (shown == "—") return null
        val parts = shown.split('.')
        return intArrayOf(
            parts.getOrNull(0)?.toIntOrNull() ?: return null,
            parts.getOrNull(1)?.toIntOrNull() ?: 0,
            parts.getOrNull(2)?.toIntOrNull() ?: 0,
        )
    }
    val have = parse(installed) ?: return false
    val need = parse(latest) ?: return false
    val index = (0..2).firstOrNull { have[it] != need[it] } ?: return false
    return have[index] < need[index]
}

/** Second line of a compact Saved row: ID, short OS, when it was last seen, and its PJDesk version. */
fun savedSubtitle(computer: SavedComputer, nowMs: Long = System.currentTimeMillis(), mine: Boolean = false): String {
    val fresh = mine || computer.presenceFresh
    val whenText = if (mine || (computer.presenceFresh && computer.online)) "now" else agoShort(computer.lastSeen, nowMs)
    val version = if (fresh) shownAppVersion(computer.appVersion) else ""
    return listOf(formatSavedId(computer.id), osShort(computer), whenText, version)
        .filter { it.isNotBlank() }
        .joinToString(" · ")
}

/** A local wipe stays empty until the next explicit sign-in (`preferAccount`). */
fun mergeSavedFromAccount(localCleared: Boolean, preferAccount: Boolean): Boolean =
    !localCleared || preferAccount

/**
 * Existing installs that already show a saved list keep syncing.
 * An empty list stays unset until the next sign-in chooses no.
 * `stored` is `yes`, `no`, or empty.
 */
fun savedSyncChoiceOnUpdate(stored: String, savedCount: Int): String {
    if (stored == "yes" || stored == "no") return stored
    return if (savedCount > 0) "yes" else ""
}

/** Confirm-dialog sentence. `count` is the saved list on this device. */
fun savedComputerCountLine(count: Int): String {
    val noun = if (count == 1) "computer" else "computers"
    return "This computer has $count saved $noun."
}

fun savedMatches(computer: SavedComputer, query: String): Boolean {
    if (query.isBlank()) return true
    val q = query.lowercase()
    val qid = q.filter { !it.isWhitespace() }
    val id = computer.id.filter { !it.isWhitespace() }.lowercase()
    return computerTitle(computer).lowercase().contains(q)
        || computer.hostname.lowercase().contains(q)
        || computer.displayName.lowercase().contains(q)
        || computer.name.lowercase().contains(q)
        || (qid.isNotEmpty() && id.contains(qid))
        || computer.osFamily.lowercase().contains(q)
        || computer.osVersion.lowercase().contains(q)
        || osDetail(computer).lowercase().contains(q)
        || computer.consoleUser.lowercase().contains(q)
}

/**
 * Saved tab order. Recent-tab rows stay in [recentIds] order.
 * Online first puts online computers ahead of offline ones, then by name.
 */
fun arrangeSaved(
    computers: List<SavedComputer>,
    recentIds: List<String>,
    sort: SavedSort,
    filter: SavedFilter,
    query: String,
    recentOnly: Boolean,
    selfId: String = "",
    nowMs: Long = System.currentTimeMillis(),
): List<SavedComputer> {
    val self = selfId.filter { it.isDigit() }
    val source = if (recentOnly) {
        recentIds.map { id -> computers.find { it.id == id } ?: SavedComputer(id = id) }
    } else {
        computers
    }
    val filtered = source.filter { computer ->
        val digits = computer.id.filter { it.isDigit() }
        if (presenceStale(computer, nowMs) && digits != self) return@filter false
        val keep = when (filter) {
            SavedFilter.All -> true
            SavedFilter.Online -> computer.online
            SavedFilter.Offline -> !computer.online
        }
        keep && savedMatches(computer, query)
    }
    val byName = compareBy<SavedComputer, String>(String.CASE_INSENSITIVE_ORDER) { computerTitle(it) }
    val ordered = if (recentOnly) {
        filtered
    } else {
        when (sort) {
            SavedSort.Name -> filtered.sortedWith(byName)
            SavedSort.Online -> filtered.sortedWith(compareByDescending<SavedComputer> { it.online }.then(byName))
            SavedSort.Recent -> {
                val rank = recentIds.withIndex().associate { it.value to it.index }
                filtered.sortedWith(compareBy<SavedComputer> { rank[it.id] ?: Int.MAX_VALUE }.then(byName))
            }
        }
    }
    if (self.isEmpty()) return ordered
    val (mine, others) = ordered.partition { it.id.filter { ch -> ch.isDigit() } == self }
    return others + mine
}

/** `user@host` on a big tile. The host is the computer name when no hostname is known. */
fun tileCaption(computer: SavedComputer): String {
    val host = computer.hostname.trim().ifBlank {
        computer.displayName.trim().ifBlank {
            computer.nickname().ifBlank { formatSavedId(computer.id) }
        }
    }
    val user = computer.consoleUser.trim()
    return if (user.isEmpty()) host else "$user@$host"
}

/**
 * Computers heard on the LAN, in beacon order. A saved record supplies the
 * platform. This phone is left out. The same ID is kept once.
 */
fun discoveredComputers(
    sights: List<LanSight>,
    saved: List<SavedComputer>,
    selfId: String,
): List<SavedComputer> {
    val self = selfId.filter { it.isDigit() }
    val out = LinkedHashMap<String, SavedComputer>()
    for (sight in sights) {
        val id = sight.id.filter { it.isDigit() }
        if (id.length != 9 || id == self) continue
        val known = saved.find { it.id.filter { ch -> ch.isDigit() } == id }
        val host = sight.name.trim()
        out[id] = if (known == null) {
            SavedComputer(id = id, hostname = host, online = true)
        } else {
            known.copy(online = true, hostname = known.hostname.ifBlank { host })
        }
    }
    return out.values.toList()
}
