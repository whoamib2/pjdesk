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

/** "3d ago" from a unix-second or unix-millis stamp. Empty when the time is unknown. */
fun agoShort(lastSeen: Long, nowMs: Long): String {
    if (lastSeen <= 0L) return ""
    val seenMs = if (lastSeen < 10_000_000_000L) lastSeen * 1000 else lastSeen
    val mins = ((nowMs - seenMs).coerceAtLeast(0L)) / 60_000L
    return when {
        mins < 1 -> "now"
        mins < 60 -> "${mins}m ago"
        mins < 60 * 24 -> "${mins / 60}h ago"
        else -> "${mins / (60 * 24)}d ago"
    }
}

/** Second line of a compact Saved row: ID, short OS, and when it was last seen. */
fun savedSubtitle(computer: SavedComputer, nowMs: Long = System.currentTimeMillis()): String {
    val whenText = if (computer.online) "now" else agoShort(computer.lastSeen, nowMs)
    return listOf(formatSavedId(computer.id), osShort(computer), whenText)
        .filter { it.isNotBlank() }
        .joinToString(" · ")
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
): List<SavedComputer> {
    val source = if (recentOnly) {
        recentIds.map { id -> computers.find { it.id == id } ?: SavedComputer(id = id) }
    } else {
        computers
    }
    val filtered = source.filter { computer ->
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
    val self = selfId.filter { it.isDigit() }
    if (self.isEmpty()) return ordered
    val (mine, others) = ordered.partition { it.id.filter { ch -> ch.isDigit() } == self }
    return others + mine
}
