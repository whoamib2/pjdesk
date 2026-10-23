package us.spader.pjdesk

import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import us.spader.pjdesk.ui.PermanentPasswordFields
import us.spader.pjdesk.ui.PermanentPasswordSection
import us.spader.pjdesk.ui.PjTheme
import us.spader.pjdesk.ui.UnattendedAccessRow

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [33], qualifiers = "w400dp-h800dp")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class PermanentPasswordUiTest {
    @get:Rule
    val rule = createComposeRule()

    @Test
    fun notSetShowsSetAndTheDialogFields() {
        rule.setContent {
            PjTheme(theme = "dark") {
                PermanentPasswordSection(hasPermanent = false, onSave = { _, _ -> true }, onRemove = {})
            }
        }
        rule.onNodeWithText("No permanent password is set.").assertIsDisplayed()
        rule.onNodeWithText("Set").assertIsDisplayed()
    }

    @Test
    fun dialogFieldsShowThePasswordConfirmHintAndSave() {
        rule.mainClock.autoAdvance = false
        rule.setContent {
            PjTheme(theme = "dark") {
                PermanentPasswordFields(
                    password = "",
                    onPassword = {},
                    confirm = "",
                    onConfirm = {},
                    issue = null,
                )
            }
        }
        rule.onNodeWithText("Password").assertIsDisplayed()
        rule.onNodeWithText("Confirm password").assertIsDisplayed()
        rule.onNodeWithText("At least 8 characters. This is not your PJDesk account password.").assertIsDisplayed()
    }

    @Test
    fun aSavedPasswordOffersChangeAndRemove() {
        var removed = false
        rule.setContent {
            PjTheme(theme = "dark") {
                PermanentPasswordSection(
                    hasPermanent = true,
                    onSave = { _, _ -> true },
                    onRemove = { removed = true },
                )
            }
        }
        rule.onNodeWithText("A permanent password is set.").assertIsDisplayed()
        rule.onNodeWithText("Change").assertIsDisplayed()
        rule.onNodeWithText("Remove").assertIsDisplayed().performClick()
        rule.runOnIdle { check(removed) }
    }

    @Test
    fun unattendedAccessStartsOffAndCanBeSwitched() {
        var on = false
        rule.setContent {
            PjTheme(theme = "dark") {
                UnattendedAccessRow(on = on, onChange = { on = it })
            }
        }
        rule.onNodeWithText("Unattended access (accept automatically)").assertIsDisplayed().performClick()
        rule.runOnIdle { check(on) }
    }
}
