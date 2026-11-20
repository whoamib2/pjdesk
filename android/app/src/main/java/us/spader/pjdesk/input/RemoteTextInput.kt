package us.spader.pjdesk.input

/**
 * Turns a text field's committed value into backspaces plus inserted text for
 * a remote computer. IME composition (Gboard suggestions) is ignored until the
 * text is committed, so predictive text does not send leftover characters.
 */
class RemoteTextInput {
    private var committed = ""

    fun reset() {
        committed = ""
    }

    /**
     * When [composing] is true the IME still owns the text; returns null and
     * leaves the baseline alone. A committed update is compared to the last
     * sent string.
     */
    fun onField(text: String, composing: Boolean): RemoteTextDelta? {
        if (composing) return null
        return commit(text)
    }

    fun commit(now: String): RemoteTextDelta {
        val old = scalars(committed)
        val next = scalars(now)
        var i = 0
        val n = minOf(old.size, next.size)
        while (i < n && old[i] == next[i]) i++
        val backspaces = old.size - i
        val text = next.subList(i, next.size).joinToString("")
        committed = now
        return RemoteTextDelta(backspaces, text)
    }

    companion object {
        /** PC set-1 scan code for a letter or space, for sticky modifier combos. */
        fun scanForChar(ch: Char): Int? {
            if (ch == ' ') return 0x39
            val lower = ch.lowercaseChar()
            if (lower in 'a'..'z') return LETTER_SCAN[lower - 'a']
            return null
        }

        private val LETTER_SCAN = intArrayOf(
            0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24,
            0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14,
            0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
        )
    }
}

data class RemoteTextDelta(
    val backspaces: Int = 0,
    val text: String = "",
) {
    val isEmpty: Boolean get() = backspaces <= 0 && text.isEmpty()
}

private fun scalars(text: String): List<String> {
    val out = ArrayList<String>(text.length)
    var i = 0
    while (i < text.length) {
        val cp = text.codePointAt(i)
        out.add(String(Character.toChars(cp)))
        i += Character.charCount(cp)
    }
    return out
}
