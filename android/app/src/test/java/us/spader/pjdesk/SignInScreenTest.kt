package us.spader.pjdesk

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import us.spader.pjdesk.core.SignInTrace
import us.spader.pjdesk.ui.PjTheme
import us.spader.pjdesk.ui.SignInScreen

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [33], qualifiers = "w400dp-h900dp")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class SignInScreenTest {
    @get:Rule
    val rule = createComposeRule()

    @Test
    fun signInButtonLogsBeforeTheNetworkAndCopyDiagnosticsShowsTheSteps() {
        val trace = SignInTrace()
        val logged = ArrayList<String>()
        var submitted = false
        var copied = ""
        rule.setContent {
            PjTheme(theme = "dark") {
                SignInScreen(
                    email = "ben@spader.us",
                    password = "secret-password",
                    code = "",
                    useCode = false,
                    busy = false,
                    error = "Could not reach the PJDesk server.",
                    onEmail = {},
                    onPassword = {},
                    onCode = {},
                    onUseCode = {},
                    serverAddress = "192.168.0.193:21200",
                    signInTrace = trace,
                    onSignInTrace = { logged.add(it) },
                    onSubmit = { submitted = true },
                    onCopyDiagnostics = { copied = trace.copyText() },
                )
            }
        }
        rule.onNodeWithText("Copy diagnostics").performScrollTo().assertIsDisplayed()
        rule.onNodeWithText("Sign in").performScrollTo().performClick()
        assertTrue(submitted)
        assertTrue(logged.first().contains("step=screen"))
        assertTrue(logged.first().contains("192.168.0.193:21200"))
        assertTrue(trace.copyText().contains("step=screen"))
        assertFalse(trace.copyText().contains("secret-password"))
        assertFalse(logged.any { it.contains("secret-password") })
        rule.onNodeWithText("Copy diagnostics").performScrollTo().performClick()
        assertTrue(copied.contains("step=screen"))
        assertFalse(copied.contains("secret-password"))
    }
}
