package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.input.RemoteTextInput

class RemoteTextInputTest {
    @Test
    fun committedCharactersAreTheDeltaOnly() {
        val input = RemoteTextInput()
        val first = input.commit("a")
        assertEquals(0, first.backspaces)
        assertEquals("a", first.text)
        val second = input.commit("ab")
        assertEquals(0, second.backspaces)
        assertEquals("b", second.text)
        assertTrue(input.commit("ab").isEmpty)
    }

    @Test
    fun compositionIsIgnoredUntilCommit() {
        val input = RemoteTextInput()
        assertNull(input.onField("hel", composing = true))
        val committed = input.onField("hello", composing = false)!!
        assertEquals(0, committed.backspaces)
        assertEquals("hello", committed.text)
    }

    @Test
    fun replacingAWordSendsBackspacesThenTheNewTail() {
        val input = RemoteTextInput()
        input.commit("hello")
        val changed = input.commit("help")
        assertEquals(2, changed.backspaces)
        assertEquals("p", changed.text)
    }

    @Test
    fun backspaceRemovesTheLastCharacter() {
        val input = RemoteTextInput()
        input.commit("ab")
        val deleted = input.commit("a")
        assertEquals(1, deleted.backspaces)
        assertEquals("", deleted.text)
    }

    @Test
    fun unicodeAndSymbolsAreSentAsText() {
        val input = RemoteTextInput()
        val accented = input.commit("é")
        assertEquals("é", accented.text)
        val symbols = input.commit("é@#\$%&*()")
        assertEquals(0, symbols.backspaces)
        assertEquals("@#\$%&*()", symbols.text)
        assertNull(RemoteTextInput.scanForChar('@'))
        assertNull(RemoteTextInput.scanForChar('é'))
        assertEquals(0x2E, RemoteTextInput.scanForChar('c'))
        assertEquals(0x2E, RemoteTextInput.scanForChar('C'))
        assertEquals(0x39, RemoteTextInput.scanForChar(' '))
    }

    @Test
    fun resetClearsTheBaseline() {
        val input = RemoteTextInput()
        input.commit("ab")
        input.reset()
        val again = input.commit("ab")
        assertEquals("ab", again.text)
        assertEquals(0, again.backspaces)
    }
}
