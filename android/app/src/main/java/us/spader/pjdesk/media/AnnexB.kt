package us.spader.pjdesk.media

/**
 * Turns MediaCodec's AVC output into the Annex-B access units PJDesk already
 * carries. The Windows client decodes those bytes with OpenH264, which reads
 * start codes and does not read AVCC length prefixes or an avcC blob.
 */
object AnnexB {
    private val START = byteArrayOf(0, 0, 0, 1)
    const val IDR = 5
    const val SPS = 7

    fun isAnnexB(data: ByteArray): Boolean {
        if (data.size >= 4 && data[0] == 0.toByte() && data[1] == 0.toByte() && data[2] == 0.toByte() && data[3] == 1.toByte()) {
            return true
        }
        return data.size >= 3 && data[0] == 0.toByte() && data[1] == 0.toByte() && data[2] == 1.toByte()
    }

    /**
     * An avcC decoder-configuration record (the codec-config buffer, first byte
     * `1`) becomes Annex-B SPS and PPS. A buffer that is already Annex-B is
     * returned unchanged.
     */
    fun parameterSets(csd: ByteArray): ByteArray {
        if (csd.isEmpty() || isAnnexB(csd)) return csd
        if (csd[0] != 1.toByte() || csd.size < 7) return csd
        val out = ArrayList<Byte>(csd.size + 16)
        var i = 5
        val lengthSize = (csd[4].toInt() and 0x03) + 1
        if (lengthSize != 2 && i + 1 < csd.size) {
            // SPS and PPS lengths inside avcC are always 2 bytes, whatever the
            // sample length size is. `lengthSize` is only recorded for frames.
        }
        val numSps = csd[i].toInt() and 0x1F
        i += 1
        repeat(numSps) {
            if (i + 2 > csd.size) return out.toByteArray()
            val len = readBe(csd, i, 2)
            i += 2
            appendNal(out, csd, i, len)
            i += len
        }
        if (i >= csd.size) return out.toByteArray()
        val numPps = csd[i].toInt() and 0xFF
        i += 1
        repeat(numPps) {
            if (i + 2 > csd.size) return out.toByteArray()
            val len = readBe(csd, i, 2)
            i += 2
            appendNal(out, csd, i, len)
            i += len
        }
        return out.toByteArray()
    }

    fun avccToAnnexB(data: ByteArray, lengthSize: Int = 4): ByteArray {
        if (data.isEmpty() || isAnnexB(data)) return data
        val size = lengthSize.coerceIn(1, 4)
        val out = ArrayList<Byte>(data.size + 16)
        var i = 0
        while (i + size <= data.size) {
            val len = readBe(data, i, size)
            i += size
            if (len <= 0 || i + len > data.size) break
            appendNal(out, data, i, len)
            i += len
        }
        return if (out.isEmpty()) data else out.toByteArray()
    }

    fun nalType(annexB: ByteArray): Int {
        val units = split(annexB)
        if (units.isEmpty()) return -1
        return units[0][0].toInt() and 0x1F
    }

    class Converter {
        private var sets: ByteArray = ByteArray(0)

        fun config(csd: ByteArray) {
            val parsed = parameterSets(csd)
            if (parsed.isNotEmpty()) sets = parsed
        }

        /** Appends a second codec-config buffer, such as a bare PPS NAL. */
        fun add(csd: ByteArray) {
            val parsed = when {
                csd.isEmpty() -> return
                isAnnexB(csd) || csd[0] == 1.toByte() -> parameterSets(csd)
                else -> {
                    val wrapped = ByteArray(4 + csd.size)
                    wrapped[3] = 1
                    csd.copyInto(wrapped, 4)
                    wrapped
                }
            }
            if (parsed.isEmpty()) return
            sets = if (sets.isEmpty()) parsed else sets + parsed
        }

        fun frame(data: ByteArray, keyframe: Boolean): ByteArray {
            val body = if (isAnnexB(data)) data else avccToAnnexB(data)
            // An IDR is a sync point even when the codec forgot BUFFER_FLAG_KEY_FRAME.
            // Repeat SPS/PPS unless this access unit already carries them.
            val sync = keyframe || containsNal(body, IDR)
            if (!sync || sets.isEmpty() || containsNal(body, SPS)) return body
            val out = ByteArray(sets.size + body.size)
            sets.copyInto(out)
            body.copyInto(out, sets.size)
            return out
        }
    }

    /** True when [data] is Annex-B and contains a NAL of [type] (5 = IDR, 7 = SPS). */
    fun containsNal(data: ByteArray, type: Int): Boolean {
        if (!isAnnexB(data)) return false
        return split(data).any { it.isNotEmpty() && (it[0].toInt() and 0x1F) == type }
    }

    private fun split(data: ByteArray): List<ByteArray> {
        val marks = ArrayList<Int>()
        var i = 0
        while (i + 3 < data.size) {
            if (data[i] == 0.toByte() && data[i + 1] == 0.toByte() && data[i + 2] == 1.toByte()) {
                marks.add(if (i > 0 && data[i - 1] == 0.toByte()) i - 1 else i)
                i += 3
            } else {
                i += 1
            }
        }
        if (marks.isEmpty()) return emptyList()
        val nals = ArrayList<ByteArray>(marks.size)
        for (m in marks.indices) {
            val start = marks[m]
            val code = if (start + 3 < data.size && data[start + 2] == 1.toByte()) 3 else 4
            val from = start + code
            val to = if (m + 1 < marks.size) marks[m + 1] else data.size
            if (from < to) nals.add(data.copyOfRange(from, to))
        }
        return nals
    }

    private fun readBe(data: ByteArray, offset: Int, size: Int): Int {
        var n = 0
        for (b in 0 until size) n = (n shl 8) or (data[offset + b].toInt() and 0xFF)
        return n
    }

    private fun appendNal(out: ArrayList<Byte>, data: ByteArray, offset: Int, len: Int) {
        if (len <= 0 || offset < 0 || offset + len > data.size) return
        for (b in START) out.add(b)
        for (i in 0 until len) out.add(data[offset + i])
    }
}
