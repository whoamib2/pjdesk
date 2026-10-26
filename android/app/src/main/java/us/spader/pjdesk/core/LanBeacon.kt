package us.spader.pjdesk.core

/** A computer beacon already checked. The host is the packet's source address. */
data class LanSight(
    val id: String,
    val name: String,
    val host: String,
    val port: Int,
    val seenAt: Long = 0L,
)

data class LanBeaconBody(
    val id: String,
    val port: Int,
    val name: String,
)

private const val MAX_BODY = 400

/**
 * Reads a PJDESKLB discovery packet. The body is the same postcard layout the
 * desktop sends: protocol, id, direct port, name, and a 32-byte fingerprint.
 * A server beacon (PJDESKSV) is not a computer.
 */
fun decodeLanBeacon(packet: ByteArray): LanBeaconBody? {
    if (packet.size < 10) return null
    if (!packet.copyOfRange(0, 8).contentEquals(MAGIC)) return null
    val len = ((packet[8].toInt() and 0xFF) shl 8) or (packet[9].toInt() and 0xFF)
    if (len > MAX_BODY || packet.size < 10 + len) return null
    val body = packet.copyOfRange(10, 10 + len)
    var index = 0
    fun varint(): Long? {
        var result = 0L
        var shift = 0
        while (index < body.size && shift <= 28) {
            val bite = body[index].toInt() and 0xFF
            index += 1
            result = result or ((bite and 0x7F).toLong() shl shift)
            if (bite and 0x80 == 0) return result
            shift += 7
        }
        return null
    }
    fun text(limit: Int): String? {
        val count = varint()?.toInt() ?: return null
        if (count < 0 || count > limit || index + count > body.size) return null
        val value = body.copyOfRange(index, index + count).toString(Charsets.UTF_8)
        index += count
        return value
    }
    val proto = varint() ?: return null
    if (proto !in 0..65535) return null
    val id = text(16) ?: return null
    val port = varint() ?: return null
    if (port !in 1..65535) return null
    val name = text(64) ?: return null
    if (index + 32 > body.size) return null
    return LanBeaconBody(id = id, port = port.toInt(), name = name)
}

/** Private, link-local, loopback, or shared (carrier-grade NAT) addresses. */
fun acceptLanHost(host: String): Boolean {
    val bare = host.removePrefix("/").removePrefix("[").substringBefore("%").substringBefore("]")
    if (bare.contains(':')) {
        val lower = bare.lowercase()
        return lower == "::1" || lower.startsWith("fe80:") || lower.startsWith("fc") || lower.startsWith("fd")
    }
    val parts = bare.split('.')
    if (parts.size != 4) return false
    val nums = parts.map { it.toIntOrNull() ?: return false }
    if (nums.any { it !in 0..255 }) return false
    val a = nums[0]
    val b = nums[1]
    if (a == 0 || a >= 224) return false
    return a == 127 || a == 10 || (a == 172 && b in 16..31) || (a == 192 && b == 168) ||
        (a == 169 && b == 254) || (a == 100 && b in 64..127)
}

private val MAGIC = "PJDESKLB".toByteArray(Charsets.US_ASCII)
