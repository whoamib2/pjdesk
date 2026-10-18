package us.spader.pjdesk

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.flow.MutableStateFlow
import us.spader.pjdesk.a11y.PjAccessibilityService
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.media.ScreenDecoder
import us.spader.pjdesk.service.HostService
import us.spader.pjdesk.ui.HomeScreen
import us.spader.pjdesk.ui.OnboardingScreen
import us.spader.pjdesk.ui.PjTheme
import us.spader.pjdesk.ui.SavedScreen
import us.spader.pjdesk.ui.SessionScreen
import us.spader.pjdesk.ui.SettingsScreen

class MainActivity : ComponentActivity() {
    private val captureReady = MutableStateFlow(HostService.capturing)
    private var pendingAccept = false
    private var decoder: ScreenDecoder? = null

    private val notifications = registerForActivityResult(ActivityResultContracts.RequestPermission()) { }

    private val projection = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val data = result.data
        if (result.resultCode == RESULT_OK && data != null) {
            HostService.capture(this, result.resultCode, data)
            captureReady.value = true
            if (pendingAccept) {
                pendingAccept = false
                app.engine.accept()
            }
        } else if (pendingAccept) {
            pendingAccept = false
            app.engine.decline()
        }
    }

    private val app get() = application as PjApp

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        HostService.arm(this)
        val engine = app.engine
        engine.onVideo = { data, w, h, key -> decoder?.submit(data, w, h, key) }
        setContent {
            val theme = remember { mutableStateOf(app.store.theme) }
            val state by engine.state.collectAsStateWithLifecycle()
            val snackbar = remember { SnackbarHostState() }
            var tab by remember { mutableIntStateOf(0) }
            var connectId by remember { mutableStateOf("") }
            var connectPassword by remember { mutableStateOf("") }
            var query by remember { mutableStateOf("") }
            var saved by remember { mutableStateOf(app.store.saved()) }
            var showOnboarding by remember { mutableStateOf(!app.store.onboarded) }
            val capture by captureReady.collectAsState()
            LaunchedEffect(state.error) {
                val text = state.error ?: return@LaunchedEffect
                snackbar.showSnackbar(text)
                engine.dismissError()
            }
            LaunchedEffect(state.hosting) {
                HostService.controlled(this@MainActivity, state.hosting)
            }
            LaunchedEffect(state.session?.id, state.session?.platform, state.session?.version) {
                val session = state.session ?: return@LaunchedEffect
                if (session.id.isBlank()) return@LaunchedEffect
                app.store.remember(
                    SavedComputer(
                        id = session.id,
                        name = session.peer.ifBlank { session.id },
                        osFamily = session.platform,
                        osVersion = session.version,
                    ),
                )
                saved = app.store.saved()
            }
            PjTheme(theme.value) {
                val session = state.session
                if (session != null) {
                    val sessionDecoder = remember {
                        ScreenDecoder { _, _ -> engine.keyframe() }
                    }
                    DisposableEffect(sessionDecoder) {
                        decoder = sessionDecoder
                        onDispose {
                            sessionDecoder.stop()
                            if (decoder === sessionDecoder) decoder = null
                        }
                    }
                    SessionScreen(
                        session = session,
                        onGesture = engine::gesture,
                        onMouse = engine::mouse,
                        onText = engine::text,
                        onSpecial = engine::special,
                        onKey = engine::key,
                        onQuality = engine::quality,
                        onDisplay = { index -> engine.display(index, session.layout) },
                        onEnd = { engine.stop() },
                        decoder = sessionDecoder,
                    )
                } else if (showOnboarding) {
                    OnboardingScreen(
                        notificationsGranted = notificationsGranted(),
                        accessibilityEnabled = PjAccessibilityService.enabled(this),
                        captureReady = capture,
                        onNotifications = { askNotifications() },
                        onAccessibility = { startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) },
                        onCapture = { askProjection(acceptAfter = false) },
                        onDone = {
                            app.store.onboarded = true
                            showOnboarding = false
                        },
                    )
                } else {
                    Scaffold(
                        snackbarHost = { SnackbarHost(snackbar) },
                        bottomBar = {
                            NavigationBar {
                                NavigationBarItem(selected = tab == 0, onClick = { tab = 0 }, icon = { Text("Home") }, label = { Text("Home") })
                                NavigationBarItem(selected = tab == 1, onClick = { tab = 1 }, icon = { Text("Saved") }, label = { Text("Saved") })
                                NavigationBarItem(selected = tab == 2, onClick = { tab = 2 }, icon = { Text("Settings") }, label = { Text("Settings") })
                            }
                        },
                    ) { padding ->
                        androidx.compose.foundation.layout.Box(Modifier.padding(padding)) {
                            when (tab) {
                                0 -> HomeScreen(
                                    state = state,
                                    connectId = connectId,
                                    connectPassword = connectPassword,
                                    onConnectId = { connectId = it },
                                    onConnectPassword = { connectPassword = it },
                                    onConnect = { engine.connect(connectId, connectPassword) },
                                    onCopyId = { copy(state.idFormatted) },
                                    onCopyPassword = { copy(state.password) },
                                    onRegenerate = engine::regenerate,
                                    onStop = { HostService.stop(this@MainActivity) },
                                )
                                1 -> SavedScreen(
                                    computers = saved,
                                    query = query,
                                    onQuery = { query = it },
                                    onOpen = {
                                        connectId = it.id
                                        tab = 0
                                    },
                                    onForget = {
                                        app.store.forget(it.id)
                                        saved = app.store.saved()
                                    },
                                )
                                else -> SettingsScreen(
                                    state = state,
                                    theme = theme.value,
                                    onTheme = {
                                        theme.value = it
                                        app.store.theme = it
                                    },
                                    onServer = engine::setServer,
                                    onResetFingerprint = engine::resetFingerprint,
                                    onPermanent = engine::setPermanent,
                                    onApproval = engine::setApproval,
                                )
                            }
                        }
                    }
                }
                state.incoming?.let { incoming ->
                    AlertDialog(
                        onDismissRequest = { engine.decline() },
                        title = { Text("Allow control?") },
                        text = { Text("${incoming.name.ifBlank { "Someone" }} (${incoming.ip}) wants to control this phone.") },
                        confirmButton = {
                            TextButton(onClick = { askProjection(acceptAfter = true) }) { Text("Allow") }
                        },
                        dismissButton = {
                            TextButton(onClick = { engine.decline() }) { Text("Decline") }
                        },
                    )
                }
            }
        }
    }

    override fun onResume() {
        super.onResume()
        captureReady.value = HostService.capturing
    }

    override fun onDestroy() {
        if (isFinishing) decoder?.stop()
        super.onDestroy()
    }

    private fun notificationsGranted(): Boolean {
        if (Build.VERSION.SDK_INT < 33) return true
        return ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
    }

    private fun askNotifications() {
        if (Build.VERSION.SDK_INT >= 33) notifications.launch(Manifest.permission.POST_NOTIFICATIONS)
    }

    private fun askProjection(acceptAfter: Boolean) {
        if (HostService.capturing) {
            if (acceptAfter) app.engine.accept()
            return
        }
        pendingAccept = acceptAfter
        val manager = getSystemService(MediaProjectionManager::class.java)
        projection.launch(manager.createScreenCaptureIntent())
    }

    private fun copy(text: String) {
        val clipboard = getSystemService(ClipboardManager::class.java)
        clipboard.setPrimaryClip(ClipData.newPlainText("pjdesk", text))
    }
}
