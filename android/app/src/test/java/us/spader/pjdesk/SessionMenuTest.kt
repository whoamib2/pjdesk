package us.spader.pjdesk

import androidx.compose.ui.test.assertHeightIsAtLeast
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.assertWidthIsAtLeast
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithContentDescription
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.unit.dp
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import us.spader.pjdesk.core.DisplayInfo
import us.spader.pjdesk.core.SessionInfo
import us.spader.pjdesk.ui.PjTheme
import us.spader.pjdesk.ui.SessionScreen

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [33], qualifiers = "w400dp-h800dp")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class SessionMenuTest {
    @get:Rule
    val rule = createComposeRule()

    @Test
    fun gearAndBubbleOpenSettingsAboveThePicture() {
        rule.setContent {
            PjTheme(theme = "dark") {
                SessionScreen(
                    session = SessionInfo(
                        peer = "Cabin Desk",
                        id = "902488771",
                        path = "relay",
                        platform = "windows",
                        version = "11",
                        layout = true,
                        displays = listOf(DisplayInfo(0, "Main", 1920, 1080, true)),
                        frameW = 1920,
                        frameH = 1080,
                    ),
                    onGesture = {},
                    onMouse = { _, _, _, _, _ -> },
                    onText = {},
                    onSpecial = {},
                    onKey = { _, _ -> },
                    onQuality = {},
                    onDisplay = {},
                    onEnd = {},
                    decoder = null,
                )
            }
        }
        rule.onNodeWithContentDescription("Session menu").assertIsDisplayed()
        rule.onNodeWithContentDescription("Session menu").assertWidthIsAtLeast(56.dp)
        rule.onNodeWithContentDescription("Session menu").assertHeightIsAtLeast(56.dp)
        rule.onNodeWithContentDescription("Session settings").assertIsDisplayed()
        rule.onNodeWithContentDescription("Session settings").assertWidthIsAtLeast(56.dp)
        rule.onNodeWithContentDescription("Session menu").performClick()
        rule.waitForIdle()
        rule.onNodeWithText("Input").assertIsDisplayed()
        rule.onNodeWithText("Touch").assertIsDisplayed()
        rule.onNodeWithText("Display").performScrollTo().assertIsDisplayed()
        rule.onNodeWithText("Keyboard").performScrollTo().assertIsDisplayed()
        rule.onNodeWithText("Blank the other screen").performScrollTo().assertIsDisplayed()
        rule.onNodeWithContentDescription("Close session settings").performClick()
        rule.onNodeWithContentDescription("Session settings").performClick()
        rule.onNodeWithText("Mouse").assertIsDisplayed()
    }
}
