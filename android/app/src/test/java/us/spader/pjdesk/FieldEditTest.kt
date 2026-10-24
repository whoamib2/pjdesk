package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Test
import us.spader.pjdesk.a11y.FieldEdit

class FieldEditTest {
    @Test
    fun backspaceDeletesTheSelectionOrTheCharacterBeforeTheCursor() {
        val end = FieldEdit.apply("ab", 2, 2, FieldEdit.BACKSPACE) as FieldEdit.Outcome.Replace
        assertEquals("a", end.text)
        assertEquals(1, end.cursor)
        val middle = FieldEdit.apply("hello", 1, 4, FieldEdit.BACKSPACE) as FieldEdit.Outcome.Replace
        assertEquals("ho", middle.text)
        assertEquals(1, middle.cursor)
        val empty = FieldEdit.apply("a", 0, 0, FieldEdit.BACKSPACE) as FieldEdit.Outcome.Replace
        assertEquals("a", empty.text)
        assertEquals(0, empty.cursor)
    }

    @Test
    fun arrowsMoveByCharacterAndByLine() {
        val left = FieldEdit.apply("ab", 2, 2, FieldEdit.LEFT) as FieldEdit.Outcome.Replace
        assertEquals(1, left.cursor)
        val right = FieldEdit.apply("ab", 0, 0, FieldEdit.RIGHT) as FieldEdit.Outcome.Replace
        assertEquals(1, right.cursor)
        val up = FieldEdit.apply("one\ntwo", 5, 5, FieldEdit.UP) as FieldEdit.Outcome.Replace
        assertEquals(4, up.cursor)
        val down = FieldEdit.apply("one\ntwo", 1, 1, FieldEdit.DOWN) as FieldEdit.Outcome.Replace
        assertEquals(3, down.cursor)
    }

    @Test
    fun pasteInsertsAtTheCursorAndCommandsStaySingleCharacters() {
        val pasted = FieldEdit.apply("ab", 1, 1, "xy\nz") as FieldEdit.Outcome.Replace
        assertEquals("axy\nzb", pasted.text)
        assertEquals(5, pasted.cursor)
        assertEquals(FieldEdit.Outcome.Enter, FieldEdit.apply("ab", 1, 1, FieldEdit.ENTER))
        assertEquals(FieldEdit.Outcome.Tab, FieldEdit.apply("ab", 1, 1, FieldEdit.TAB))
        val newline = FieldEdit.insert("ab", 1, 1, "\n")
        assertEquals("a\nb", newline.text)
        assertEquals(2, newline.cursor)
    }
}
