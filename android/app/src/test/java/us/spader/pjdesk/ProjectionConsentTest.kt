package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.a11y.ConsentNode
import us.spader.pjdesk.a11y.ConsentStep
import us.spader.pjdesk.a11y.ProjectionConsent

class ProjectionConsentTest {
    @Test
    fun android14OpensTheSpinnerWhenASingleAppIsSelected() {
        val step = ProjectionConsent.decide(
            listOf(
                mode("A single app"),
                button("android:id/button1", "Start now"),
                button("android:id/button2", "Cancel"),
            ),
        )
        assertEquals(ConsentStep.Click(0, "open share mode"), step)
    }

    @Test
    fun theDropdownTapsShareEntireScreenBeforeStart() {
        val step = ProjectionConsent.decide(
            listOf(
                mode("A single app"),
                row("A single app"),
                row("Share entire screen"),
                button("android:id/button1", "Share screen"),
            ),
        )
        assertEquals(ConsentStep.Click(2, "entire screen"), step)
    }

    @Test
    fun entireScreenThenStartNow() {
        val step = ProjectionConsent.decide(
            listOf(
                mode("Entire screen"),
                button("android:id/button1", "Start now"),
                button("android:id/button2", "Cancel"),
            ),
        )
        assertEquals(ConsentStep.Click(1, "share"), step)
    }

    @Test
    fun shareScreenIsThePositiveButton() {
        val step = ProjectionConsent.decide(
            listOf(
                mode("Entire screen"),
                button("android:id/button1", "Share screen"),
            ),
        )
        assertEquals(ConsentStep.Click(1, "share"), step)
    }

    @Test
    fun aSpanishEntireScreenRowIsChosen() {
        val step = ProjectionConsent.decide(
            listOf(
                mode("Una sola app"),
                row("Una sola app"),
                row("Pantalla completa"),
            ),
        )
        assertEquals(ConsentStep.Click(2, "entire screen"), step)
    }

    @Test
    fun anUnknownEntireRowIsTheOneThatIsNotSingleApp() {
        val step = ProjectionConsent.decide(
            listOf(
                mode("A single app"),
                row("A single app"),
                row("Hele scherm"),
            ),
        )
        assertEquals(ConsentStep.Click(2, "entire screen"), step)
    }

    @Test
    fun cancelAndASettingsWindowAreNotTheShareButton() {
        assertFalse(ProjectionConsent.looksLikeConsent(listOf(ConsentNode(packageName = "com.android.settings", text = "Start"))))
        val step = ProjectionConsent.decide(
            listOf(
                mode("Entire screen"),
                button("android:id/button2", "Cancel"),
                button("android:id/button1", "Start"),
            ),
        )
        assertEquals(ConsentStep.Click(2, "share"), step)
        assertTrue(ProjectionConsent.isConfirm("Start now"))
        assertFalse(ProjectionConsent.isConfirm("Start sharing?"))
        assertFalse(ProjectionConsent.isSingleApp("Entire screen"))
    }

    @Test
    fun anEmptyTreeWaits() {
        assertEquals(ConsentStep.Wait, ProjectionConsent.decide(emptyList()))
    }

    private fun mode(selected: String) = ConsentNode(
        packageName = ProjectionConsent.SYSTEM_UI,
        className = "android.widget.Spinner",
        windowClass = "com.android.systemui.mediaprojection.permission.MediaProjectionPermissionActivity",
        viewId = "com.android.systemui:id/screen_share_mode_options",
        text = selected,
        subtree = selected,
        clickable = true,
        enabled = true,
    )

    private fun row(label: String) = ConsentNode(
        packageName = ProjectionConsent.SYSTEM_UI,
        className = "android.widget.TextView",
        viewId = "android:id/text1",
        text = label,
        subtree = label,
        clickable = true,
        enabled = true,
    )

    private fun button(id: String, label: String) = ConsentNode(
        packageName = ProjectionConsent.SYSTEM_UI,
        className = "android.widget.Button",
        windowClass = "com.android.systemui.mediaprojection.permission.MediaProjectionPermissionActivity",
        viewId = id,
        text = label,
        subtree = label,
        clickable = true,
        enabled = true,
    )
}
