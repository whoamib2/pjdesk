package us.spader.pjdesk.a11y

/**
 * Turns a PC keystroke into a change of the focused field.
 * Command characters are single and exact. Anything longer, including a paste, is inserted.
 */
object FieldEdit {
    const val BACKSPACE = "\u0008"
    const val ENTER = "\n"
    const val TAB = "\t"
    const val LEFT = "\uE000"
    const val RIGHT = "\uE001"
    const val UP = "\uE002"
    const val DOWN = "\uE003"

    sealed class Outcome {
        data class Replace(val text: String, val cursor: Int) : Outcome()
        data object Enter : Outcome()
        data object Tab : Outcome()
    }

    fun apply(text: String, start: Int, end: Int, input: String): Outcome {
        val (from, to) = bounds(text, start, end)
        return when (input) {
            ENTER -> Outcome.Enter
            TAB -> Outcome.Tab
            BACKSPACE -> {
                if (from != to) Outcome.Replace(text.removeRange(from, to), from)
                else if (from == 0) Outcome.Replace(text, 0)
                else {
                    val cut = step(text, from, -1)
                    Outcome.Replace(text.removeRange(cut, from), cut)
                }
            }
            LEFT, UP -> Outcome.Replace(text, vertical(text, from, to, input))
            RIGHT, DOWN -> Outcome.Replace(text, vertical(text, from, to, input))
            else -> insert(text, from, to, input)
        }
    }

    fun insert(text: String, start: Int, end: Int, input: String): Outcome.Replace {
        val (from, to) = bounds(text, start, end)
        val next = text.substring(0, from) + input + text.substring(to)
        return Outcome.Replace(next, from + input.length)
    }

    private fun bounds(text: String, start: Int, end: Int): Pair<Int, Int> {
        val len = text.length
        var from = start.coerceIn(0, len)
        var to = end.coerceIn(0, len)
        if (from > to) {
            val swap = from
            from = to
            to = swap
        }
        return from to to
    }

    private fun vertical(text: String, from: Int, to: Int, input: String): Int {
        val backward = input == LEFT || input == UP
        val at = if (from != to) {
            if (backward) from else to
        } else {
            from
        }
        if (input == LEFT) return step(text, at, -1)
        if (input == RIGHT) return step(text, at, 1)
        val (lineStart, lineEnd) = lineBounds(text, at)
        if (input == UP) {
            if (at > lineStart) return lineStart
            if (lineStart == 0) return 0
            return lineBounds(text, lineStart - 1).first
        }
        if (at < lineEnd) return lineEnd
        if (lineEnd >= text.length) return text.length
        return lineBounds(text, lineEnd + 1).second
    }

    private fun lineBounds(text: String, cursor: Int): Pair<Int, Int> {
        val at = cursor.coerceIn(0, text.length)
        val start = if (at == 0) 0 else {
            val nl = text.lastIndexOf('\n', at - 1)
            if (nl < 0) 0 else nl + 1
        }
        val nl = text.indexOf('\n', at)
        val end = if (nl < 0) text.length else nl
        return start to end
    }

    private fun step(text: String, index: Int, delta: Int): Int {
        val at = index.coerceIn(0, text.length)
        return try {
            Character.offsetByCodePoints(text, at, delta).coerceIn(0, text.length)
        } catch (_: IndexOutOfBoundsException) {
            (at + delta).coerceIn(0, text.length)
        }
    }
}
