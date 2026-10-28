package us.spader.pjdesk.ui

import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SubmitKeyboardTest {
    @Test
    fun passwordAndPinFieldsShowADoneKeyThatSubmits() {
        val password = submitKeyboard()
        assertEquals(ImeAction.Done, password.imeAction)
        assertEquals(KeyboardType.Password, password.keyboardType)
        val pin = submitKeyboard(KeyboardType.NumberPassword)
        assertEquals(ImeAction.Done, pin.imeAction)
        assertEquals(KeyboardType.NumberPassword, pin.keyboardType)
    }

    @Test
    fun theRefreshSpinIsShort() {
        assertTrue(REFRESH_SPIN_MS in 250..600)
    }
}
