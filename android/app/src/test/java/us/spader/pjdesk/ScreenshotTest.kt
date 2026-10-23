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
import us.spader.pjdesk.ui.SessionScreen
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
            onConnectId = {},
            onConnect = {},
            onCopyId = {},
            onCopyPassword = {},
            onRegenerate = {},
            onStop = {},
        )
    }

    @Test
    fun saved() = shot("saved") { savedList() }

    @Test
    fun savedLight() = shot("saved-light", "light") { savedList() }

    @Composable
    private fun savedList() {
        SavedScreen(
            computers = listOf(
                SavedComputer("123456789", "Front desk", "windows", "11", hostname = "OFFICE-PC"),
                SavedComputer("555000111", "", "android", "15", hostname = "Pixel-8"),
                SavedComputer("424242424", "Studio", "macos", "15", hostname = "mac-studio"),
                SavedComputer("386626882", "386 626 882", "", ""),
            ),
            query = "",
            onQuery = {},
            onForget = {},
        )
    }

    @Test
    fun settings() = shot("servercfg-dark") {
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
    fun settingsLight() = shot("servercfg-light", "light") {
        SettingsScreen(
            state = DeskState(
                server = "office.example:21200",
                fingerprint = "12:34:56:78:9a:bc:de:f0:11:22:33:44:55:66:77:88",
                hasPermanent = true,
                approval = "prompt",
            ),
            theme = "light",
            onTheme = {},
            onServer = {},
            onResetFingerprint = {},
        )
    }

    @Test
    fun importOfferDark() = shot("import-offer-dark", "dark") { importOffer() }

    @Test
    fun importOfferLight() = shot("import-offer-light", "light") { importOffer() }

    @Composable
    private fun importOffer() {
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
            importOffer = "pjdesk-net1:paste-the-key-here",
        )
    }

    @Test
    fun networkKeyDark() = shot("netkey-dark", "dark") { networkKey() }

    @Test
    fun networkKeyLight() = shot("netkey-light", "light") { networkKey() }

    @Composable
    private fun networkKey() {
        SignInScreen(
            email = "",
            password = "",
            code = "",
            useCode = false,
            useKey = true,
            blob = "pjdesk-net1:paste-the-key-here",
            keyPass = "",
            busy = false,
            error = "",
            onEmail = {},
            onPassword = {},
            onCode = {},
            onBlob = {},
            onKeyPass = {},
            onUseCode = {},
            onUseKey = {},
            onScan = {},
            onSubmit = {},
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
        SessionScreen(
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
