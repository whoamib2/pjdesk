package us.spader.pjdesk.core

enum class SavedSort(val storage: String) {
    Online("online"),
    Name("name"),
    Recent("recent"),
    ;

    companion object {
        fun fromStorage(value: String?): SavedSort = entries.firstOrNull { it.storage == value } ?: Online
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
    if (recentOnly) return filtered
    val byName = compareBy<SavedComputer, String>(String.CASE_INSENSITIVE_ORDER) { computerTitle(it) }
    return when (sort) {
        SavedSort.Name -> filtered.sortedWith(byName)
        SavedSort.Online -> filtered.sortedWith(compareByDescending<SavedComputer> { it.online }.then(byName))
        SavedSort.Recent -> {
            val rank = recentIds.withIndex().associate { it.value to it.index }
            filtered.sortedWith(compareBy<SavedComputer> { rank[it.id] ?: Int.MAX_VALUE }.then(byName))
        }
    }
}
