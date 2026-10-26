package us.spader.pjdesk.core

/** Same twelve colors as the desktop client. The same id always picks the same one. */
private val TILE_PALETTE = intArrayOf(
    0x5C3A2E,
    0x7A2E45,
    0x5B3A78,
    0x1F4D3A,
    0x3D2F6B,
    0x6B2D4A,
    0x1E4A55,
    0x3D454D,
    0x8A5A1E,
    0x2C4A6E,
    0x2E4A32,
    0x6E3A22,
)

/** ID when there is one, otherwise the name. Spaces are ignored. */
fun tileKey(id: String, name: String): String {
    val compact = id.filter { !it.isWhitespace() }
    return if (compact.isNotEmpty()) compact.lowercase() else name.trim().lowercase()
}

fun tileIndex(key: String): Int {
    var hash = 0x811C9DC5.toInt()
    for (byte in key.toByteArray(Charsets.UTF_8)) {
        hash = hash xor (byte.toInt() and 0xFF)
        hash *= 0x01000193
    }
    val unsigned = hash.toLong() and 0xFFFFFFFFL
    return (unsigned % TILE_PALETTE.size).toInt()
}

/** Opaque RGB, without the alpha byte. */
fun tileRgb(key: String): Int = TILE_PALETTE[tileIndex(key)]
