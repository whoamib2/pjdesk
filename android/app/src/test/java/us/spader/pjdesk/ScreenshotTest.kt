package us.spader.pjdesk

import android.graphics.Bitmap
import android.graphics.Canvas
import android.view.View
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
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
import us.spader.pjdesk.core.SavedView
import us.spader.pjdesk.ui.ComputerRow
import us.spader.pjdesk.ui.HomeScreen
import us.spader.pjdesk.ui.OnboardingScreen
import us.spader.pjdesk.ui.PjBottomBar
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

    @Test
    fun savedBenDark() = shot("saved-ben-dark", "dark", check = ::tilesPresent) { shell { benSaved() } }

    @Test
    fun savedBenLight() = shot("saved-ben-light", "light", check = ::tilesPresent) { shell { benSaved() } }

    @Test
    fun savedSmallDark() = shot("saved-small-dark", "dark", check = ::tilesPresent) {
        shell { benSaved(view = SavedView.Small) }
    }

    @Test
    fun savedBigLight() = shot("saved-big-light", "light", check = ::tilesPresent) {
        shell { benSaved(view = SavedView.Big) }
    }

    @Test
    fun discoveredDark() = shot("discovered-dark", "dark", check = ::tilesPresent) {
        shell {
            benSaved(
                discovered = listOf(
                    SavedComputer("111555694", hostname = "Office PC", osFamily = "windows", osVersion = "11", online = true),
                    SavedComputer("555000111", hostname = "Pixel-8", osFamily = "android", osVersion = "15", online = true),
                ),
                initialPage = 2,
            )
        }
    }

    @Test
    fun homeRecentDark() = shot("home-recent-dark", "dark", check = ::tilesPresent) { recentBlock() }

    @Test
    fun homeRecentLight() = shot("home-recent-light", "light", check = ::tilesPresent) { recentBlock() }

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

    @OptIn(ExperimentalMaterial3Api::class)
    @Composable
    private fun shell(content: @Composable () -> Unit) {
        Scaffold(
            containerColor = MaterialTheme.colorScheme.background,
            topBar = {
                TopAppBar(
                    title = { Text("Saved computers") },
                    colors = TopAppBarDefaults.topAppBarColors(
                        containerColor = MaterialTheme.colorScheme.background,
                        titleContentColor = MaterialTheme.colorScheme.onBackground,
                    ),
                )
            },
            bottomBar = { PjBottomBar(selected = 1, onSelect = {}) },
        ) { padding ->
            Box(Modifier.padding(padding)) { content() }
        }
    }

    @Composable
    private fun benSaved(
        view: SavedView = SavedView.List,
        discovered: List<SavedComputer> = emptyList(),
        initialPage: Int = 0,
    ) {
        val now = System.currentTimeMillis() / 1000
        SavedScreen(
            computers = listOf(
                SavedComputer("692996394", "Ben Lenovo Desk Home", "windows", "11", hostname = "BENLENOVODESK", online = true),
                SavedComputer("902488771", "Cabin Desk", "windows", "11", hostname = "CABINDESK", online = false, lastSeen = now - 3600),
                SavedComputer("622775091", hostname = "BEN-SURFACE-9", osFamily = "windows", osVersion = "11", online = false, lastSeen = now - 6 * 3600),
                SavedComputer("610978221", hostname = "CHIP20", osFamily = "windows", osVersion = "11", online = true),
                SavedComputer("386626882", "Pixel 11 Pro XL", "android", "14", hostname = "Pixel", online = false, lastSeen = now - 4 * 86400),
                SavedComputer("482636105", "Ben Pixel 11", "android", "17", hostname = "Ben Pixel 11", online = true),
            ),
            query = "",
            onQuery = {},
            onForget = {},
            selfId = "482636105",
            view = view,
            discovered = discovered,
            initialPage = initialPage,
        )
    }

    @Composable
    private fun recentBlock() {
        val rows = listOf(
            SavedComputer("692996394", "Ben Lenovo Desk Home", "windows", "11", hostname = "BENLENOVODESK", online = true),
            SavedComputer("902488771", "Cabin Desk", "windows", "11", hostname = "CABINDESK", online = false),
            SavedComputer("482636105", "Ben Pixel 11", "android", "17", hostname = "Ben Pixel 11", online = true),
        )
        Column(
            Modifier
                .fillMaxSize()
                .background(MaterialTheme.colorScheme.background),
        ) {
            Text("Recent", modifier = Modifier.padding(16.dp), style = MaterialTheme.typography.titleMedium)
            rows.forEach { computer ->
                ComputerRow(
                    computer = computer,
                    mine = computer.id == "482636105",
                    showMenu = false,
                    onConnect = {},
                )
            }
        }
    }

    private fun tilesPresent(bitmap: Bitmap) {
        val palette = intArrayOf(
            0x5C3A2E, 0x7A2E45, 0x5B3A78, 0x1F4D3A, 0x3D2F6B, 0x6B2D4A,
            0x1E4A55, 0x3D454D, 0x8A5A1E, 0x2C4A6E, 0x2E4A32, 0x6E3A22,
        )
        assertTrue("colored tile", palette.any { hasColor(bitmap, it) })
        assertTrue("accent connect", hasColor(bitmap, 0x4D9BFF) || hasColor(bitmap, 0x2F6FED))
    }

    private fun hasColor(bitmap: Bitmap, rgb: Int): Boolean {
        val target = 0xFF000000.toInt() or rgb
        var y = 0
        while (y < bitmap.height) {
            var x = 0
            while (x < bitmap.width) {
                val pixel = bitmap.getPixel(x, y)
                if (near(pixel, target)) return true
                x += 3
            }
            y += 3
        }
        return false
    }

    private fun near(pixel: Int, target: Int): Boolean {
        fun ch(value: Int, shift: Int) = (value shr shift) and 0xFF
        return kotlin.math.abs(ch(pixel, 16) - ch(target, 16)) < 12 &&
            kotlin.math.abs(ch(pixel, 8) - ch(target, 8)) < 12 &&
            kotlin.math.abs(ch(pixel, 0) - ch(target, 0)) < 12
    }

    @Test
    fun discoveredLabelNarrow() = shot("discovered-narrow", "dark", width = 960, height = 1600, check = ::tilesPresent) {
        shell {
            benSaved(
                discovered = listOf(
                    SavedComputer("111555694", hostname = "Office PC", osFamily = "windows", osVersion = "11", online = true),
                ),
                initialPage = 2,
            )
        }
    }

    private fun shot(
        name: String,
        theme: String = "dark",
        width: Int = 1080,
        height: Int = 1920,
        check: ((Bitmap) -> Unit)? = null,
        content: @Composable () -> Unit,
    ) {
        val activity = Robolectric.buildActivity(ComponentActivity::class.java).setup().get()
        activity.setContent { PjTheme(theme = theme) { content() } }
        ShadowLooper.idleMainLooper()
        val view = activity.window.decorView
        view.measure(
            View.MeasureSpec.makeMeasureSpec(width, View.MeasureSpec.EXACTLY),
            View.MeasureSpec.makeMeasureSpec(height, View.MeasureSpec.EXACTLY),
        )
        view.layout(0, 0, width, height)
        ShadowLooper.idleMainLooper()
        val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
        view.draw(Canvas(bitmap))
        assertTrue("$name rendered a blank bitmap", hasInk(bitmap))
        check?.invoke(bitmap)
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
