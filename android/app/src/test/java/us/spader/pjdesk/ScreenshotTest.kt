package us.spader.pjdesk

import android.graphics.Bitmap
import android.graphics.Canvas
import android.view.View
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import org.robolectric.shadows.ShadowLooper
import us.spader.pjdesk.core.DeskState
import us.spader.pjdesk.core.DisplayInfo
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.SessionInfo
import us.spader.pjdesk.ui.HomeScreen
import us.spader.pjdesk.ui.OnboardingScreen
import us.spader.pjdesk.ui.PjTheme
import us.spader.pjdesk.ui.SavedScreen
import us.spader.pjdesk.ui.SignInScreen
import us.spader.pjdesk.ui.SessionChrome
import us.spader.pjdesk.ui.SettingsScreen
import java.io.File

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [33], qualifiers = "w360dp-h800dp")
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class ScreenshotTest {
    @Test
    fun home() = shot("home") {
        HomeScreen(
            state = DeskState(
                id = "123456789",
                idFormatted = "123 456 789",
                password = "k7mw4qpn",
                status = "Online",
                server = "office.example:21200",
                fingerprint = "ab:cd:ef",
            ),
            connectId = "",
            connectPassword = "",
            onConnectId = {},
            onConnectPassword = {},
            onConnect = {},
            onCopyId = {},
            onCopyPassword = {},
            onRegenerate = {},
            onStop = {},
        )
    }

    @Test
    fun saved() = shot("saved") {
        SavedScreen(
            computers = listOf(
                SavedComputer("123456789", "Office", "windows", "11"),
                SavedComputer("555000111", "Pixel", "android", "15"),
                SavedComputer("424242424", "Studio", "macos", "15"),
            ),
            query = "",
            onQuery = {},
            onOpen = {},
            onForget = {},
        )
    }

    @Test
    fun settings() = shot("settings") {
        SettingsScreen(
            state = DeskState(
                server = "office.example:21200",
                fingerprint = "12:34:56:78:9a:bc:de:f0:11:22:33:44:55:66:77:88",
                hasPermanent = true,
                approval = "prompt",
            ),
            theme = "dark",
            onTheme = {},
            onServer = {},
            onResetFingerprint = {},
        )
    }

    @Test
    fun signInDark() = shot("signin-dark", "dark") { signIn() }

    @Test
    fun signInLight() = shot("signin-light", "light") { signIn() }

    @Composable
    private fun signIn() {
        SignInScreen(
            email = "ben@spader.us",
            password = "",
            code = "",
            useCode = false,
            busy = false,
            error = "",
            onEmail = {},
            onPassword = {},
            onCode = {},
            onUseCode = {},
            onSubmit = {},
        )
    }

    @Test
    fun onboarding() = shot("onboarding") {
        OnboardingScreen(
            notificationsGranted = true,
            accessibilityEnabled = false,
            captureReady = false,
            onNotifications = {},
            onAccessibility = {},
            onCapture = {},
            onDone = {},
        )
    }

    @Test
    fun sessionToolbar() = shot("session") {
        Box(Modifier.fillMaxSize().background(Color.Black)) {
        SessionChrome(
            modifier = Modifier.align(Alignment.BottomCenter),
            session = SessionInfo(
                peer = "Pixel",
                id = "555000111",
                path = "direct",
                platform = "android",
                version = "15",
                layout = true,
                displays = listOf(
                    DisplayInfo(0, "Phone", 1080, 2400, true),
                    DisplayInfo(1, "External", 1920, 1080, false),
                ),
                frameW = 1080,
                frameH = 2400,
            ),
            onText = {},
            onSpecial = {},
            onKey = { _, _ -> },
            onQuality = {},
            onDisplay = {},
            onEnd = {},
        )
        }
    }

    private fun shot(name: String, theme: String = "dark", content: @Composable () -> Unit) {
        val activity = Robolectric.buildActivity(ComponentActivity::class.java).setup().get()
        activity.setContent { PjTheme(theme = theme) { content() } }
        ShadowLooper.idleMainLooper()
        val view = activity.window.decorView
        val width = 1080
        val height = 1920
        view.measure(
            View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
            View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY),
        )
        view.layout(0, 0, width, height)
        ShadowLooper.idleMainLooper()
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        view.draw(Canvas(bitmap))
        assertTrue("$name rendered a blank bitmap", hasInk(bitmap))
        val dir = File("/opt/cursor/artifacts/screenshots")
        dir.mkdirs()
        File(dir, "pjdesk-android-$name.png").outputStream().use {
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, it)
        }
    }

    private fun hasInk(bitmap: Bitmap): Boolean {
        val first = bitmap.getPixel(0, 0)
        var y = 0
        while (y < bitmap.height) {
            var x = 0
            while (x < bitmap.width) {
                if (bitmap.getPixel(x, y) != first) return true
                x += 24
            }
            y += 24
        }
        return false
    }
}
