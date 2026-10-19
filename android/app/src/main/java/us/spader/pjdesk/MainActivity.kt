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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import androidx.compose.runtime.rememberCoroutineScope
import org.json.JSONObject
import us.spader.pjdesk.core.CloudAuth
import us.spader.pjdesk.ui.SignInScreen
import java.util.UUID
import us.spader.pjdesk.a11y.PjAccessibilityService
import us.spader.pjdesk.core.permanentIssue
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
    private var pendingAccept by mutableStateOf(false)
    private var decoder: ScreenDecoder? = null

    private val notifications = registerForActivityResult(ActivityResultContracts.RequestPermission()) { }

    private val projection = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val data = result.data
        val acceptAfter = pendingAccept
        if (result.resultCode == RESULT_OK && data != null) {
            if (acceptAfter) {
                AppLog.i("screen-share consent granted for an incoming connection")
                HostService.onCapture = { ok, reason ->
                    runOnUiThread {
                        HostService.onCapture = null
                        pendingAccept = false
                        if (ok) {
                            captureReady.value = true
                            app.engine.accept()
                        } else {
                            captureReady.value = false
                            val text = reason.ifBlank { "Screen capture permission denied" }
                            app.engine.showError(text)
                            app.engine.decline(text)
                        }
                    }
                }
                HostService.capture(this, result.resultCode, data)
            } else {
                // The consent token is single-use on Android 14+. Remember that the
                // prompt works, and ask again when a session actually starts.
                pendingAccept = false
                app.store.captureTried = true
                captureReady.value = true
                AppLog.i("screen-share prompt completed during setup")
            }
        } else if (acceptAfter) {
            pendingAccept = false
            val text = "Screen capture permission denied"
            AppLog.e(text)
            app.engine.showError(text)
            app.engine.decline(text)
        }
    }

    private val app get() = application as PjApp

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        HostService.arm(this)
        captureReady.value = HostService.capturing || app.store.captureTried
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
            var account by remember { mutableStateOf(app.store.networkEmail) }
            var netEmail by remember { mutableStateOf(app.store.networkEmail) }
            var netPass by remember { mutableStateOf("") }
            var netCode by remember { mutableStateOf("") }
            var netUseCode by remember { mutableStateOf(false) }
            var netBusy by remember { mutableStateOf(false) }
            var netError by remember { mutableStateOf("") }
            var savePassword by remember { mutableStateOf(false) }
            val scope = rememberCoroutineScope()
            var showPermanent by remember { mutableStateOf(false) }
            val capture by captureReady.collectAsState()
            LaunchedEffect(account) {
                val refresh = app.store.networkRefresh()
                val device = app.store.networkDeviceId
                if (account.isBlank() || refresh.isBlank() || device.isBlank()) return@LaunchedEffect
                val session = withContext(Dispatchers.IO) {
                    runCatching { CloudAuth.refresh(refresh, device) }.getOrNull()
                } ?: return@LaunchedEffect
                engine.command(JSONObject().put("cmd", "accessToken").put("token", session.access).toString())
                if (session.address.isNotBlank()) {
                    engine.command(JSONObject().put("cmd", "server").put("address", session.address).toString())
                }
            }
            LaunchedEffect(state.error) {
                val text = state.error ?: return@LaunchedEffect
                snackbar.showSnackbar(text)
                engine.dismissError()
            }
            LaunchedEffect(state.notice) {
                val text = state.notice ?: return@LaunchedEffect
                snackbar.showSnackbar(text)
                engine.dismissNotice()
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
                } else if (account.isEmpty() && state.server.isEmpty()) {
                    SignInScreen(
                        email = netEmail,
                        password = netPass,
                        code = netCode,
                        useCode = netUseCode,
                        busy = netBusy,
                        error = netError,
                        onEmail = { netEmail = it },
                        onPassword = { netPass = it },
                        onCode = { netCode = it },
                        onUseCode = { netUseCode = it },
                        onSubmit = {
                            val device = app.store.networkDeviceId.ifBlank {
                                UUID.randomUUID().toString().also { app.store.networkDeviceId = it }
                            }
                            netBusy = true
                            netError = ""
                            scope.launch {
                                val result = withContext(Dispatchers.IO) {
                                    runCatching {
                                        if (netUseCode) CloudAuth.enroll(netCode, device, "Android")
                                        else CloudAuth.signIn(netEmail, netPass, device, "Android")
                                    }
                                }
                                netBusy = false
                                result.onSuccess { session ->
                                    val email = session.email.ifBlank { netEmail.trim() }
                                    app.store.networkEmail = email
                                    app.store.networkDeviceId = session.deviceId
                                    app.store.saveNetworkRefresh(session.refresh)
                                    engine.command(JSONObject().put("cmd", "server").put("address", session.address).toString())
                                    engine.command(JSONObject().put("cmd", "accessToken").put("token", session.access).toString())
                                    session.computers.filter { it.id.length == 9 }.forEach { app.store.remember(it) }
                                    netPass = ""
                                    account = email
                                }.onFailure { netError = it.message ?: "Could not sign in." }
                            }
                        },
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
                                    savePassword = savePassword,
                                    onSavePassword = { savePassword = it },
                                    onConnect = {
                                        val id = connectId.filter { it.isDigit() }
                                        if (savePassword && connectPassword.isNotBlank()) {
                                            app.store.saveComputerPassword(id, connectPassword)
                                        }
                                        val password = connectPassword.ifBlank { app.store.computerPassword(id) }
                                        engine.connect(connectId, password)
                                    },
                                    onCopyId = { copy(state.idFormatted) },
                                    onCopyPassword = { copy(state.password) },
                                    onRegenerate = engine::regenerate,
                                    onStop = { HostService.stop(this@MainActivity) },
                                    showPermanent = showPermanent,
                                    onShowPermanent = { showPermanent = it },
                                    onSavePermanent = { password, confirm ->
                                        val issue = permanentIssue(password, confirm)
                                        if (issue != null) {
                                            engine.showError(issue)
                                            false
                                        } else {
                                            engine.setPermanent(password.trim())
                                        }
                                    },
                                    onClearPermanent = {
                                        engine.setPermanent("")
                                        engine.setApproval("prompt")
                                    },
                                    onUnattended = { engine.setApproval(if (it) "unattended" else "prompt") },
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
                                    onRemovePassword = {
                                        app.store.saveComputerPassword(it.id, "")
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
                                    onSavePermanent = { password, confirm ->
                                        val issue = permanentIssue(password, confirm)
                                        if (issue != null) {
                                            engine.showError(issue)
                                            false
                                        } else {
                                            engine.setPermanent(password.trim())
                                        }
                                    },
                                    onClearPermanent = {
                                        engine.setPermanent("")
                                        engine.setApproval("prompt")
                                    },
                                    onUnattended = { engine.setApproval(if (it) "unattended" else "prompt") },
                                    onShareLogs = { AppLog.share(this@MainActivity) },
                                )
                            }
                        }
                    }
                }
                val incoming = state.incoming
                if (incoming != null && !pendingAccept) {
                    AlertDialog(
                        onDismissRequest = { if (!pendingAccept) engine.decline() },
                        title = { Text("Allow control?") },
                        text = { Text("${incoming.name.ifBlank { "Someone" }} (${incoming.ip}) wants to control this phone.") },
                        confirmButton = {
                            TextButton(onClick = { askProjection(acceptAfter = true) }) { Text("Allow") }
                        },
                        dismissButton = {
                            TextButton(onClick = {
                                if (!pendingAccept) engine.decline()
                            }) { Text("Decline") }
                        },
                    )
                }
            }
        }
    }

    override fun onResume() {
        super.onResume()
        captureReady.value = HostService.capturing || app.store.captureTried
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
        // Android 14 and later consume each consent on one virtual display.
        // Always ask again for a real session, including when setup already did.
        pendingAccept = acceptAfter
        if (acceptAfter) app.engine.showStatus("Waiting for screen sharing")
        AppLog.i(if (acceptAfter) "asking to share the screen for an incoming connection" else "asking to share the screen during setup")
        val manager = getSystemService(MediaProjectionManager::class.java)
        projection.launch(manager.createScreenCaptureIntent())
    }

    private fun copy(text: String) {
        val clipboard = getSystemService(ClipboardManager::class.java)
        clipboard.setPrimaryClip(ClipData.newPlainText("pjdesk", text))
    }
}
