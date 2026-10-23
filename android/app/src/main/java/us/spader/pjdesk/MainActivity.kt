@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)

package us.spader.pjdesk

import android.Manifest
import android.content.BroadcastReceiver
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.media.projection.MediaProjectionManager
import android.net.ConnectivityManager
import android.net.Network
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.autofill.AutofillManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.fragment.app.FragmentActivity
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.OutlinedTextField
import androidx.compose.ui.Alignment
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.List
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import androidx.compose.runtime.rememberCoroutineScope
import org.json.JSONObject
import us.spader.pjdesk.core.AppLockPolicy
import us.spader.pjdesk.core.BioVault
import us.spader.pjdesk.core.CloudAuth
import us.spader.pjdesk.core.CloudAuthException
import us.spader.pjdesk.core.EnginePhase
import us.spader.pjdesk.core.NetKey
import us.spader.pjdesk.core.PasswordSync
import us.spader.pjdesk.core.PinWrap
import us.spader.pjdesk.core.VaultKeyRejected
import us.spader.pjdesk.core.connectFailureText
import us.spader.pjdesk.core.connectStatusLabel
import us.spader.pjdesk.ui.ReauthScreen
import us.spader.pjdesk.ui.SecretField
import us.spader.pjdesk.ui.SetPinScreen
import us.spader.pjdesk.ui.SignInScreen
import us.spader.pjdesk.ui.UnlockScreen
import java.util.UUID
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.SavedFilter
import us.spader.pjdesk.core.SavedSort
import us.spader.pjdesk.data.SavedListPrefs
import us.spader.pjdesk.core.nickname
import us.spader.pjdesk.a11y.PjAccessibilityService
import us.spader.pjdesk.core.permanentIssue
import us.spader.pjdesk.service.RemoteControl
import us.spader.pjdesk.media.ScreenDecoder
import us.spader.pjdesk.service.HostService
import us.spader.pjdesk.ui.HomeScreen
import us.spader.pjdesk.ui.OnboardingScreen
import us.spader.pjdesk.ui.PjTheme
import us.spader.pjdesk.ui.QrScan
import us.spader.pjdesk.ui.SavedScreen
import us.spader.pjdesk.ui.SessionScreen
import us.spader.pjdesk.ui.SettingsScreen

class MainActivity : FragmentActivity() {
    private val captureReady = MutableStateFlow(HostService.capturing)
    private var pendingAccept by mutableStateOf(false)
    private var decoder: ScreenDecoder? = null

    private val notifications = registerForActivityResult(ActivityResultContracts.RequestPermission()) { }

    private val projection = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val data = result.data
        val acceptAfter = pendingAccept
        HostService.consentInProgress = false
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
    private val tempFlag = java.util.concurrent.atomic.AtomicBoolean(false)
    private val interaction = mutableIntStateOf(0)

    override fun onUserInteraction() {
        super.onUserInteraction()
        interaction.intValue++
    }

    override fun onStop() {
        super.onStop()
        if (!app.bioPrompt) app.backgroundAt = android.os.SystemClock.elapsedRealtime()
    }

    override fun onStart() {
        super.onStart()
        val away = app.backgroundAt
        app.backgroundAt = 0L
        if (away > 0L && !app.bioPrompt && app.uiUnlocked.value) {
            val delay = app.store.autoLockSeconds * 1000L
            if (AppLockPolicy.relock(android.os.SystemClock.elapsedRealtime() - away, delay)) {
                app.lockUi()
                AppLog.i("app locked after background")
            }
        }
    }

    private fun commitAutofill() {
        val mgr = getSystemService(AutofillManager::class.java) ?: return
        if (mgr.hasEnabledAutofillServices()) mgr.commit()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (app.store.temporaryActive() && !tempFlag.get()) app.store.endTemporaryVisit()
        if (app.store.accountServer.isNotBlank()) {
            app.engine.applyAccountServer(app.store.accountServer, app.store.accountFingerprint, quiet = true)
        }
        enableEdgeToEdge()
        HostService.arm(this)
        captureReady.value = HostService.capturing || app.store.captureTried
        val engine = app.engine
        engine.onVideo = { data, w, h, key -> decoder?.submit(data, w, h, key) }
        setContent {
            val theme = remember { mutableStateOf(app.store.theme) }
            val state by engine.state.collectAsStateWithLifecycle()
            val readiness by engine.readiness.collectAsStateWithLifecycle()
            val listPrefs = remember { SavedListPrefs(applicationContext) }
            val savedSort by listPrefs.sort.collectAsStateWithLifecycle(SavedSort.Recent)
            val savedFilter by listPrefs.filter.collectAsStateWithLifecycle(SavedFilter.All)
            val snackbar = remember { SnackbarHostState() }
            var tab by remember { mutableIntStateOf(0) }
            var connectId by remember { mutableStateOf("") }
            var connectPassword by remember { mutableStateOf("") }
            var query by remember { mutableStateOf("") }
            var saved by remember { mutableStateOf(app.store.saved()) }
            var recentIds by remember { mutableStateOf(app.store.recentIds()) }
            var trustedAccounts by remember { mutableStateOf(app.store.trustedAccounts()) }
            var trustedComputers by remember { mutableStateOf(app.store.trustedComputers()) }
            var showOnboarding by remember { mutableStateOf(!app.store.onboarded) }
            var account by remember { mutableStateOf(app.store.networkEmail) }
            var netEmail by remember { mutableStateOf(app.store.networkEmail) }
            var netPass by remember { mutableStateOf("") }
            var netCode by remember { mutableStateOf("") }
            var netUseCode by remember { mutableStateOf(false) }
            var netUseKey by remember { mutableStateOf(false) }
            var netBlob by remember { mutableStateOf("") }
            var netKeyPass by remember { mutableStateOf("") }
            var netScan by remember { mutableStateOf(false) }
            var netBusy by remember { mutableStateOf(false) }
            var netError by remember { mutableStateOf("") }
            var signingIn by remember { mutableStateOf(false) }
            var netOffer by remember { mutableStateOf(false) }
            var netLogin by remember { mutableStateOf("") }
            var importOffer by remember { mutableStateOf<String?>(null) }
            var importPass by remember { mutableStateOf("") }
            var importFromPaste by remember { mutableStateOf(true) }
            var savePassword by remember { mutableStateOf(false) }
            var signOutWhenDone by remember { mutableStateOf(false) }
            var changingPin by remember { mutableStateOf(false) }
            var pinRejects by remember { mutableIntStateOf(0) }
            var lockBusy by remember { mutableStateOf(false) }
            var lockError by remember { mutableStateOf("") }
            var vaultRejected by remember { mutableStateOf(false) }
            val uiUnlocked by app.uiUnlocked.collectAsStateWithLifecycle()
            var temporarySession by remember { mutableStateOf(false) }
            var memoryRefresh by remember { mutableStateOf("") }
            var enrolledFor by remember { mutableStateOf("") }
            var accountComputers by remember { mutableStateOf(emptyList<us.spader.pjdesk.core.SavedComputer>()) }
            var tempWarn by remember { mutableStateOf(false) }
            var tempLeft by remember { mutableIntStateOf(0) }
            var lastActivity by remember { mutableLongStateOf(android.os.SystemClock.elapsedRealtime()) }
            val bumps = interaction.intValue
            val scope = rememberCoroutineScope()
            fun signOutTemporary() {
                tempFlag.set(false)
                temporarySession = false
                memoryRefresh = ""
                app.store.endTemporaryVisit()
                app.clearVaultKey()
                engine.command(org.json.JSONObject().put("cmd", "accessToken").put("token", "").toString())
                account = ""
                saved = app.store.saved()
                tempWarn = false
                enrolledFor = ""
                accountComputers = emptyList()
            }
            fun publishPermanent(id: String) {
                val digits = id.filter { it.isDigit() }
                if (digits.length != 9) return
                if (!app.store.stampPassword(digits, app.store.permanent)) return
                app.store.permanentPublished = System.currentTimeMillis() / 1000
            }
            fun adoptVaultPermanent(id: String) {
                val digits = id.filter { it.isDigit() }
                if (digits.length != 9) return
                val entry = app.store.vaultEntries().find { it.id == digits } ?: return
                if (entry.updated <= app.store.permanentPublished) return
                if (entry.password != app.store.permanent) {
                    if (!engine.setPermanent(entry.password, quiet = true)) return
                }
                app.store.permanentPublished = maxOf(app.store.permanentPublished, entry.updated)
            }
            fun syncPasswords(upload: Boolean) {
                val key = app.vaultKey ?: return
                val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                val device = app.store.networkDeviceId
                if (refresh.isBlank() || device.isBlank()) return
                scope.launch {
                    val changed = try {
                        withContext(Dispatchers.IO) { PasswordSync.pull(app.store, key, refresh, device, upload) }
                    } catch (e: VaultKeyRejected) {
                        vaultRejected = true
                        app.lockUi()
                        lockError = e.message ?: "Saved passwords need your account password again."
                        false
                    }
                    if (changed) saved = app.store.saved()
                    adoptVaultPermanent(state.id)
                }
            }
            fun savePermanent(password: String, confirm: String): Boolean {
                val issue = permanentIssue(password, confirm)
                if (issue != null) {
                    engine.showError(issue)
                    return false
                }
                if (!engine.setPermanent(password.trim())) return false
                publishPermanent(state.id)
                syncPasswords(true)
                return true
            }
            fun removePermanent() {
                if (!engine.setPermanent("")) return
                engine.setApproval("prompt")
                publishPermanent(state.id)
                syncPasswords(true)
            }
            fun rememberAccountServer(address: String, fingerprint: String, lan: List<String>, publicIp: String) {
                if (address.isBlank()) return
                app.store.accountServer = address
                app.store.accountFingerprint = fingerprint
                app.store.accountLan = lan.joinToString(",")
                app.store.accountPublicIp = publicIp
                engine.applyAccountServer(address, fingerprint, quiet = true)
                AppLog.i(
                    "account server $address lan=${lan.joinToString(",").ifBlank { "none" }} " +
                        "public=${publicIp.ifBlank { "none" }} fingerprint=${if (fingerprint.isBlank()) "missing" else "set"}",
                )
            }
            fun holdDerivedKey(email: String, password: String, upload: Boolean) {
                if (email.isBlank() || password.isBlank()) return
                scope.launch {
                    val key = withContext(Dispatchers.IO) {
                        runCatching { CloudAuth.vaultKey(email, password) }.getOrNull()
                    } ?: return@launch
                    app.holdVaultKey(key)
                    syncPasswords(upload)
                }
            }
            fun applySignedIn(
                session: us.spader.pjdesk.core.Session,
                emailHint: String,
                keyBlob: String?,
                temporaryNow: Boolean,
                enrollOnlyNow: Boolean,
                notice: String? = null,
                accountPassword: String = "",
            ) {
                val pjdeskId = state.id.filter { it.isDigit() }
                if (enrollOnlyNow && pjdeskId.length != 9) {
                    netBusy = false
                    netError = "This computer does not have an ID yet. Wait until Your ID appears, then try again."
                    return
                }
                val uploadLocal = !temporaryNow && !enrollOnlyNow
                val hostname = engine.deviceLabel()
                val display = app.store.computerName
                val osVersion = Build.VERSION.RELEASE ?: ""
                scope.launch {
                    val result = withContext(Dispatchers.IO) {
                        runCatching {
                            val list = us.spader.pjdesk.core.CloudAuth.prepareEnroll(
                                session.computers,
                                if (uploadLocal) app.store.saved() else emptyList(),
                                uploadLocal,
                                pjdeskId,
                                hostname,
                                display,
                                osVersion,
                            )
                            val next = if (session.refresh.isBlank()) {
                                session
                            } else {
                                refreshAccount(
                                    session.refresh,
                                    session.deviceId,
                                    us.spader.pjdesk.core.CloudAuth.computersJson(list),
                                )
                            }
                            next.copy(email = next.email.ifBlank { session.email.ifBlank { emailHint.trim() } })
                        }
                    }
                    netBusy = false
                    result.onSuccess { enrolled ->
                        val who = enrolled.email.ifBlank { emailHint.trim() }
                        netPass = ""
                        netKeyPass = ""
                        netCode = ""
                        netBlob = ""
                        importPass = ""
                        netError = ""
                        netOffer = false
                        netLogin = ""
                        importOffer = null
                        commitAutofill()
                        if (enrollOnlyNow) {
                            signingIn = false
                            scope.launch {
                                snackbar.showSnackbar("Added this computer to $who. Nothing from that sign-in was kept on this computer.")
                            }
                            return@onSuccess
                        }
                        app.store.networkDeviceId = enrolled.deviceId
                        if (temporaryNow) {
                            app.store.beginTemporaryVisit()
                            tempFlag.set(true)
                            temporarySession = true
                            memoryRefresh = enrolled.refresh
                            lastActivity = android.os.SystemClock.elapsedRealtime()
                            app.store.mergeCloud(enrolled.computers)
                        } else {
                            val first = app.store.networkEmail.isBlank()
                            app.store.mergeCloud(enrolled.computers, preferAccount = first)
                            app.store.networkEmail = who
                            app.store.saveNetworkRefresh(enrolled.refresh)
                            if (!keyBlob.isNullOrBlank()) {
                                us.spader.pjdesk.core.NetKey.blobIn(keyBlob)?.let { app.store.networkKey = it }
                            }
                            app.markUnlocked()
                        }
                        accountComputers = enrolled.computers
                        enrolledFor = if (pjdeskId.length == 9) pjdeskId else ""
                        rememberAccountServer(enrolled.address, enrolled.fingerprint, enrolled.lan, enrolled.publicIp)
                        engine.command(org.json.JSONObject().put("cmd", "accessToken").put("token", enrolled.access).toString())
                        account = who
                        signingIn = false
                        saved = app.store.saved()
                        if (!enrollOnlyNow) holdDerivedKey(who, accountPassword, upload = !temporaryNow)
                        if (temporaryNow) {
                            scope.launch { snackbar.showSnackbar("Signed in until you close PJDesk") }
                        } else if (!notice.isNullOrBlank()) {
                            scope.launch { snackbar.showSnackbar(notice) }
                        }
                    }.onFailure { err ->
                        val cloud = err as? us.spader.pjdesk.core.CloudAuthException
                        netOffer = cloud != null && cloud.code == "no_server" && us.spader.pjdesk.core.CloudAuth.offerCurrentServer(
                            if (cloud.role.isBlank()) "owner" else cloud.role,
                            state.server,
                            state.fingerprint,
                        )
                        netLogin = if (netOffer) cloud?.loginToken.orEmpty() else ""
                        netError = err.message ?: "Could not sign in."
                    }
                }
            }
            var askedPassword by remember { mutableStateOf("") }
            var promptBeforeConnect by remember { mutableStateOf(false) }
            var connecting by remember { mutableStateOf(false) }
            var connectError by remember { mutableStateOf<String?>(null) }
            var offlineRetry by remember { mutableStateOf<SavedComputer?>(null) }
            var lastConnectMs by remember { mutableLongStateOf(0L) }
            var connectGen by remember { mutableIntStateOf(0) }
            var askChoice by remember { mutableStateOf(false) }
            var askWaiting by remember { mutableStateOf(false) }
            var remote by remember { mutableStateOf(app.store.remoteGlobal()) }
            var peerOnly by remember { mutableStateOf(false) }
            var peerRemote by remember { mutableStateOf<us.spader.pjdesk.core.RemoteOptions?>(null) }
            val sessionRemote = if (peerOnly && peerRemote != null) peerRemote!! else remote
            var fileNames by remember { mutableStateOf(listOf<String>()) }
            var clipEcho by remember { mutableStateOf("") }
            val clipSync = remember { mutableStateOf(false) }
            clipSync.value = sessionRemote.clipboardSync
            val sendFile = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
                if (uri == null) return@rememberLauncherForActivityResult
                scope.launch {
                    val name = (uri.lastPathSegment ?: "file").substringAfterLast('/').filter { ch ->
                        ch.isLetterOrDigit() || ch == '.' || ch == '-' || ch == '_'
                    }.take(80).ifBlank { "file" }
                    val dir = java.io.File(cacheDir, "pjdesk-send").apply { mkdirs() }
                    val dest = java.io.File(dir, name)
                    val copied = withContext(Dispatchers.IO) {
                        runCatching {
                            contentResolver.openInputStream(uri)?.use { input ->
                                dest.outputStream().use { input.copyTo(it) }
                            }
                            dest.length() > 0L
                        }.getOrDefault(false)
                    }
                    if (copied) engine.sendFile(dest.absolutePath)
                    else snackbar.showSnackbar("That file cannot be sent")
                }
            }
            fun saveSessionRemote(next: us.spader.pjdesk.core.RemoteOptions) {
                if (peerOnly) {
                    val id = state.session?.id.orEmpty()
                    peerRemote = next
                    if (id.length == 9) app.store.saveRemoteOverride(id, next)
                } else {
                    remote = next
                    app.store.saveRemoteGlobal(next)
                }
            }
            fun setPeerOnly(on: Boolean) {
                val id = state.session?.id.orEmpty()
                if (on && id.length == 9) {
                    val seed = peerRemote ?: sessionRemote
                    peerRemote = seed
                    peerOnly = true
                    app.store.saveRemoteOverride(id, seed)
                } else {
                    peerOnly = false
                    peerRemote = null
                    if (id.length == 9) app.store.saveRemoteOverride(id, null)
                }
            }
            var sessionIdleWarn by remember { mutableStateOf(false) }
            var screenOff by remember { mutableStateOf(false) }
            var sessionBackground by remember { mutableStateOf(false) }
            fun launchConnect(id: String, name: String, password: String, askPerson: Boolean = false) {
                val now = android.os.SystemClock.elapsedRealtime()
                if (connecting && now - lastConnectMs < 700) {
                    AppLog.i("Connect ignored, already starting $id")
                    return
                }
                lastConnectMs = now
                promptBeforeConnect = false
                connecting = true
                connectError = null
                offlineRetry = null
                val digits = id.filter { it.isDigit() }
                app.store.touchRecent(digits)
                recentIds = app.store.recentIds()
                val label = connectStatusLabel(name, digits)
                val gen = connectGen + 1
                connectGen = gen
                AppLog.i("$label password=${if (askPerson) "ask-person" else if (password.isEmpty()) "none" else "present"}")
                if (engine.readiness.value.phase != EnginePhase.Ready) engine.showStatus("Starting PJDesk…") else engine.showStatus(label)
                scope.launch {
                    val problem = engine.awaitReady(15_000)
                    if (gen != connectGen) return@launch
                    if (problem != null) {
                        connecting = false
                        connectError = problem
                        AppLog.e("connect failed: $problem")
                        return@launch
                    }
                    engine.showStatus(label)
                    engine.connect(digits, if (askPerson) "" else password, askPerson, name)
                }
            }
            fun beginConnect(rawId: String, ignoreOffline: Boolean = false) {
                val digits = rawId.filter { it.isDigit() }
                val computer = saved.firstOrNull { it.id == digits }
                val name = connectComputerName(saved, digits)
                val savedPassword = app.store.computerPassword(digits)
                val knownOffline = computer != null && !computer.online && (
                    computer.lastSeen > 0 ||
                        computer.hostname.isNotBlank() ||
                        computer.displayName.isNotBlank() ||
                        computer.osFamily.isNotBlank()
                    )
                AppLog.i(
                    "Connect tapped id=$digits name=${name.ifBlank { "(none)" }} online=${computer?.online} " +
                        "password=${if (savedPassword.isEmpty()) "none" else "saved"} " +
                        "server=${if (state.server.isBlank()) "missing" else "set"}",
                )
                connectId = digits
                if (digits.length != 9) {
                    connecting = false
                    connectError = "That isn't a 9-digit PJDesk ID."
                    AppLog.e(connectError.orEmpty())
                    return
                }
                val knownServer = state.server.ifBlank { app.store.accountServer }
                if (state.server.isBlank() && knownServer.isNotBlank()) {
                    engine.applyAccountServer(knownServer, app.store.accountFingerprint, quiet = true)
                }
                if (knownServer.isBlank()) {
                    val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                    val device = app.store.networkDeviceId
                    if (account.isNotBlank() && refresh.isNotBlank() && device.isNotBlank()) {
                        connecting = true
                        connectError = null
                        engine.showStatus("Getting your PJDesk server…")
                        AppLog.i("Connect fetching the account server")
                        val computers = if (temporarySession) null else CloudAuth.computersJson(app.store.saved())
                        scope.launch {
                            val fetched = withContext(Dispatchers.IO) {
                                runCatching { refreshAccount(refresh, device, computers) }
                                    .onFailure { AppLog.e("account refresh failed: ${it.message}") }
                                    .getOrNull()
                            }
                            if (fetched == null || fetched.address.isBlank()) {
                                connecting = false
                                connectError = "This phone has no server address yet. Sign in again, or set the server in Settings."
                                AppLog.e(connectError.orEmpty())
                                return@launch
                            }
                            rememberAccountServer(fetched.address, fetched.fingerprint, fetched.lan, fetched.publicIp)
                            engine.command(JSONObject().put("cmd", "accessToken").put("token", fetched.access).toString())
                            beginConnect(rawId, ignoreOffline)
                        }
                        return
                    }
                    connecting = false
                    connectError = "This phone has no server address yet. Sign in again, or set the server in Settings."
                    AppLog.e(connectError.orEmpty())
                    return
                }
                if (knownOffline && !ignoreOffline) {
                    connecting = false
                    offlineRetry = computer
                    val who = name.ifBlank { computer?.hostname?.ifBlank { null } ?: "That computer" }
                    connectError = "$who is offline."
                    AppLog.i(connectError.orEmpty())
                    return
                }
                if (savedPassword.isEmpty()) {
                    askChoice = true
                    promptBeforeConnect = false
                    askedPassword = ""
                    savePassword = true
                    AppLog.i("Connect offering a password or an ask for ${name.ifBlank { digits }}")
                    return
                }
                launchConnect(digits, name, savedPassword)
            }
            val cameraPermission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
                if (granted) netScan = true else netError = "Camera permission is needed to scan a network key."
            }
            fun clipboardText(): String {
                val clip = getSystemService(ClipboardManager::class.java).primaryClip ?: return ""
                if (clip.itemCount == 0) return ""
                return clip.getItemAt(0).coerceToText(this@MainActivity)?.toString().orEmpty()
            }
            fun notePaste(value: String): Boolean {
                val blob = NetKey.blobIn(value) ?: return false
                importOffer = blob
                importPass = ""
                importFromPaste = true
                return true
            }
            fun applyImported(blob: String, passphrase: String, bound: Boolean) {
                val device = app.store.networkDeviceId.ifBlank {
                    UUID.randomUUID().toString().also { app.store.networkDeviceId = it }
                }
                netBusy = true
                netError = ""
                val email = netEmail
                val pass = netPass
                scope.launch {
                    val result = withContext(Dispatchers.IO) {
                        runCatching {
                            CloudAuth.redeem(
                                blob,
                                passphrase,
                                device,
                                "Android",
                                if (bound) email else "",
                                if (bound) pass else "",
                            )
                        }
                    }
                    result.onSuccess { session ->
                        applySignedIn(session, email, blob, signOutWhenDone, false, "Import server configuration successfully", if (bound) pass else "")
                    }.onFailure {
                        netBusy = false
                        val message = it.message ?: "Could not sign in."
                        netError = message
                        if (message.contains("specific account")) {
                            netUseKey = true
                            netBlob = blob
                            netKeyPass = passphrase
                        }
                    }
                }
            }
            var showPermanent by remember { mutableStateOf(false) }
            var controlEnabled by remember { mutableStateOf(PjAccessibilityService.enabled(this@MainActivity)) }
            var controlBound by remember { mutableStateOf(RemoteControl.service != null) }
            var controlDismissed by remember { mutableStateOf(false) }
            LaunchedEffect(Unit) {
                while (true) {
                    val enabled = PjAccessibilityService.enabled(this@MainActivity)
                    val bound = RemoteControl.service != null
                    controlEnabled = enabled
                    controlBound = bound
                    engine.setControl(enabled && bound)
                    kotlinx.coroutines.delay(1_500)
                }
            }
            LaunchedEffect(state.hosting) {
                if (!state.hosting) controlDismissed = false
            }
            LaunchedEffect(state.id, account, uiUnlocked) {
                val digits = state.id.filter { it.isDigit() }
                if (digits.length != 9 || app.store.permanentPublished != 0L) return@LaunchedEffect
                if (app.store.permanent.isEmpty()) return@LaunchedEffect
                if (temporarySession) return@LaunchedEffect
                val key = app.vaultKey
                if (account.isNotBlank() && key == null) return@LaunchedEffect
                val refresh = app.store.networkRefresh()
                val device = app.store.networkDeviceId
                if (key != null && refresh.isNotBlank() && device.isNotBlank()) {
                    val changed = withContext(Dispatchers.IO) {
                        runCatching { PasswordSync.pull(app.store, key, refresh, device, false) }.getOrDefault(false)
                    }
                    if (changed) saved = app.store.saved()
                    adoptVaultPermanent(digits)
                    if (app.store.permanentPublished != 0L) return@LaunchedEffect
                }
                if (app.store.computerPassword(digits).isNotEmpty()) return@LaunchedEffect
                if (app.store.permanent.isEmpty()) return@LaunchedEffect
                publishPermanent(digits)
                syncPasswords(true)
            }
            DisposableEffect(Unit) {
                val cm = getSystemService(ConnectivityManager::class.java)
                val callback = object : ConnectivityManager.NetworkCallback() {
                    override fun onAvailable(network: Network) {
                        val address = app.store.accountServer
                        if (address.isNotBlank()) engine.applyAccountServer(address, app.store.accountFingerprint, quiet = true)
                    }

                    override fun onLinkPropertiesChanged(network: Network, linkProperties: android.net.LinkProperties) {
                        val address = app.store.accountServer
                        if (address.isNotBlank()) engine.applyAccountServer(address, app.store.accountFingerprint, quiet = true)
                    }
                }
                runCatching { cm.registerDefaultNetworkCallback(callback) }
                onDispose { runCatching { cm.unregisterNetworkCallback(callback) } }
            }
            val capture by captureReady.collectAsState()
            LaunchedEffect(account, memoryRefresh, temporarySession) {
                val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                val device = app.store.networkDeviceId
                if (!temporarySession && state.server.isBlank() && app.store.accountServer.isNotBlank()) {
                    engine.applyAccountServer(app.store.accountServer, app.store.accountFingerprint, quiet = true)
                }
                if (account.isBlank() || refresh.isBlank() || device.isBlank()) return@LaunchedEffect
                val computers = if (temporarySession) null else CloudAuth.computersJson(app.store.saved())
                val session = withContext(Dispatchers.IO) {
                    val mine = CloudAuth.publicIp()
                    if (mine.isNotBlank()) app.store.myPublicIp = mine
                    runCatching { refreshAccount(refresh, device, computers) }
                        .onFailure { AppLog.e("account refresh failed: ${it.message}") }
                        .getOrNull()
                }
                if (session == null) {
                    if (!temporarySession && app.store.accountServer.isNotBlank()) {
                        engine.applyAccountServer(app.store.accountServer, app.store.accountFingerprint, quiet = true)
                    }
                    return@LaunchedEffect
                }
                engine.command(JSONObject().put("cmd", "accessToken").put("token", session.access).toString())
                if (!temporarySession) {
                    rememberAccountServer(session.address, session.fingerprint, session.lan, session.publicIp)
                    app.store.mergeCloud(session.computers)
                    saved = app.store.saved()
                    if (app.vaultKey != null) syncPasswords(true)
                }
            }
            LaunchedEffect(bumps, state.session?.id) {
                if (temporarySession) lastActivity = android.os.SystemClock.elapsedRealtime()
            }
            LaunchedEffect(temporarySession) {
                if (!temporarySession) {
                    tempWarn = false
                    return@LaunchedEffect
                }
                while (temporarySession && tempFlag.get()) {
                    if (state.session != null) lastActivity = android.os.SystemClock.elapsedRealtime()
                    val limit = app.store.tempIdleMinutes.coerceIn(1, 24 * 60) * 60_000L
                    val leftMs = limit - (android.os.SystemClock.elapsedRealtime() - lastActivity)
                    if (leftMs <= 0L) {
                        signOutTemporary()
                        snackbar.showSnackbar("Signed out")
                        break
                    }
                    tempWarn = leftMs <= 60_000L
                    tempLeft = ((leftMs + 999) / 1000L).toInt()
                    kotlinx.coroutines.delay(if (tempWarn) 250 else 1000)
                }
            }
            LaunchedEffect(state.id, account, temporarySession) {
                val id = state.id.filter { it.isDigit() }
                if (account.isBlank() || id.length != 9 || enrolledFor == id) return@LaunchedEffect
                val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                val device = app.store.networkDeviceId
                if (refresh.isBlank() || device.isBlank()) return@LaunchedEffect
                val uploadLocal = !temporarySession
                val local = if (uploadLocal) app.store.saved() else emptyList()
                val known = accountComputers
                val hostname = engine.deviceLabel()
                val display = app.store.computerName
                val osVersion = Build.VERSION.RELEASE ?: ""
                val ok = withContext(Dispatchers.IO) {
                    runCatching {
                        val list = CloudAuth.prepareEnroll(known, local, uploadLocal, id, hostname, display, osVersion)
                        refreshAccount(refresh, device, CloudAuth.computersJson(list))
                    }.isSuccess
                }
                if (ok) enrolledFor = id
            }
            LaunchedEffect(state.error) {
                val text = state.error ?: return@LaunchedEffect
                val friendly = connectFailureText(text, connectComputerName(saved, connectId))
                if (friendly == null) {
                    AppLog.i("connect attempt ended quietly: $text")
                    engine.dismissError()
                    return@LaunchedEffect
                }
                if (connecting || askWaiting) {
                    connectError = friendly
                    offlineRetry = null
                    connecting = false
                    askWaiting = false
                    AppLog.e("connect failed: $friendly")
                } else {
                    snackbar.showSnackbar(friendly)
                }
                engine.dismissError()
            }
            LaunchedEffect(state.askPassword, state.askRejected, state.askNote) {
                if (!state.askPassword) return@LaunchedEffect
                connecting = false
                promptBeforeConnect = false
                saved = app.store.saved()
                AppLog.i("password needed rejected=${state.askRejected}")
            }
            LaunchedEffect(state.session?.id) {
                if (state.session != null) {
                    connecting = false
                    promptBeforeConnect = false
                    askChoice = false
                    askWaiting = false
                    connectError = null
                    sessionIdleWarn = false
                }
            }
            val lifecycleOwner = LocalLifecycleOwner.current
            DisposableEffect(lifecycleOwner) {
                val observer = LifecycleEventObserver { _, event ->
                    when (event) {
                        Lifecycle.Event.ON_STOP -> sessionBackground = true
                        Lifecycle.Event.ON_START -> sessionBackground = false
                        else -> {}
                    }
                }
                lifecycleOwner.lifecycle.addObserver(observer)
                onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
            }
            DisposableEffect(Unit) {
                val receiver = object : BroadcastReceiver() {
                    override fun onReceive(context: Context?, intent: Intent?) {
                        screenOff = intent?.action == Intent.ACTION_SCREEN_OFF
                    }
                }
                val filter = IntentFilter().apply {
                    addAction(Intent.ACTION_SCREEN_OFF)
                    addAction(Intent.ACTION_SCREEN_ON)
                }
                registerReceiver(receiver, filter)
                onDispose { unregisterReceiver(receiver) }
            }
            DisposableEffect(Unit) {
                engine.onFiles = { fileNames = it }
                engine.onClipboard = { text ->
                    if (clipSync.value && text.isNotBlank() && text != clipEcho) {
                        clipEcho = text
                        getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("pjdesk", text))
                    }
                }
                onDispose {
                    engine.onFiles = null
                    engine.onClipboard = null
                }
            }
            DisposableEffect(state.session?.id, sessionRemote.clipboardSync) {
                if (state.session == null || !sessionRemote.clipboardSync) {
                    return@DisposableEffect onDispose {}
                }
                val clipboard = getSystemService(ClipboardManager::class.java)
                val listener = ClipboardManager.OnPrimaryClipChangedListener {
                    val text = clipboardText()
                    if (text.isNotBlank() && text != clipEcho) {
                        clipEcho = text
                        engine.paste(text)
                    }
                }
                clipboard.addPrimaryClipChangedListener(listener)
                onDispose { clipboard.removePrimaryClipChangedListener(listener) }
            }
            LaunchedEffect(state.session?.id) {
                fileNames = emptyList()
                val id = state.session?.id.orEmpty()
                val over = if (id.length == 9) app.store.remoteOverride(id) else null
                peerOnly = over != null
                peerRemote = over
            }
            LaunchedEffect(state.session?.id, sessionRemote.showStats) {
                if (state.session == null || !sessionRemote.showStats) return@LaunchedEffect
                while (true) {
                    engine.ping(System.nanoTime() / 1_000L)
                    kotlinx.coroutines.delay(2_000)
                }
            }
            LaunchedEffect(state.session?.id, bumps, sessionRemote.sessionIdleMin) {
                val minutes = sessionRemote.sessionIdleMin
                if (minutes <= 0 || state.session == null) {
                    sessionIdleWarn = false
                    return@LaunchedEffect
                }
                val started = android.os.SystemClock.elapsedRealtime()
                val limit = minutes * 60_000L
                val warnAt = (limit - 10_000L).coerceAtLeast(0L)
                while (state.session != null) {
                    val idle = android.os.SystemClock.elapsedRealtime() - started
                    val foreground = !sessionBackground && !screenOff
                    sessionIdleWarn = foreground && idle >= warnAt && idle < limit
                    if (idle >= limit) {
                        sessionIdleWarn = false
                        askWaiting = false
                        engine.stopView("idle")
                        break
                    }
                    kotlinx.coroutines.delay(500)
                }
            }
            LaunchedEffect(askWaiting) {
                if (!askWaiting) return@LaunchedEffect
                kotlinx.coroutines.delay(60_000)
                if (askWaiting && state.session == null) {
                    askWaiting = false
                    connecting = false
                    engine.stopView("cancelled")
                    connectError = "No answer"
                }
            }
            LaunchedEffect(state.notice) {
                val text = state.notice ?: return@LaunchedEffect
                snackbar.showSnackbar(text)
                engine.dismissNotice()
            }
            LaunchedEffect(state.hosting, state.hostWho) {
                HostService.controlled(this@MainActivity, state.hosting, state.hostWho)
            }
            LaunchedEffect(account) {
                engine.command(org.json.JSONObject().put("cmd", "account").put("email", account).toString())
            }
            LaunchedEffect(state.session?.id, state.session?.peer, state.session?.platform, state.session?.version) {
                val session = state.session ?: return@LaunchedEffect
                if (session.id.isBlank()) return@LaunchedEffect
                app.store.noteRemote(session.id, session.peer, session.platform, session.version)
                app.store.touchRecent(session.id)
                saved = app.store.saved()
                recentIds = app.store.recentIds()
                val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                val device = app.store.networkDeviceId
                if (!temporarySession && refresh.isNotBlank() && device.isNotBlank()) {
                    withContext(Dispatchers.IO) {
                        runCatching { refreshAccount(refresh, device, CloudAuth.computersJson(app.store.saved())) }
                    }
                }
            }
            fun tryBio() {
                val stored = app.store.bioWrap
                if (stored.isBlank() || !BioVault.available(this@MainActivity)) return
                app.bioPrompt = true
                BioVault.openWithAuth(
                    this@MainActivity,
                    stored,
                    onKey = { key ->
                        app.bioPrompt = false
                        app.holdVaultKey(key)
                        app.store.pinFails = 0
                        lockError = ""
                        app.markUnlocked()
                        syncPasswords(!temporarySession)
                    },
                    onCancel = { app.bioPrompt = false },
                    onError = {
                        app.bioPrompt = false
                        lockError = it
                    },
                )
            }
            fun submitPin(pin: String) {
                val salt = app.store.pinSaltBytes()
                val wrapped = app.store.pinWrap
                if (salt == null || wrapped.isBlank()) return
                lockBusy = true
                lockError = ""
                scope.launch {
                    val key = withContext(Dispatchers.IO) { PinWrap.unwrap(wrapped, pin, salt) }
                    lockBusy = false
                    if (key == null) {
                        val fails = app.store.pinFails + 1
                        app.store.pinFails = fails
                        pinRejects++
                        lockError = if (fails >= AppLockPolicy.MAX_FAILS) {
                            ""
                        } else {
                            "Wrong PIN. ${AppLockPolicy.MAX_FAILS - fails} left."
                        }
                        AppLog.i("app pin rejected fails=$fails")
                        return@launch
                    }
                    app.store.pinFails = 0
                    app.holdVaultKey(key)
                    lockError = ""
                    app.markUnlocked()
                    syncPasswords(!temporarySession)
                }
            }
            fun finishPin(pin: String, password: String, enableBio: Boolean) {
                val email = netEmail.ifBlank { account }.trim()
                lockBusy = true
                lockError = ""
                scope.launch {
                    val result = withContext(Dispatchers.IO) {
                        runCatching {
                            val existing = if (password.isBlank()) app.vaultKey?.copyOf() else null
                            if (existing != null) {
                                existing to null
                            } else {
                                val device = app.store.networkDeviceId.ifBlank {
                                    UUID.randomUUID().toString().also { app.store.networkDeviceId = it }
                                }
                                val signed = CloudAuth.signIn(email, password, device, "Android")
                                CloudAuth.vaultKey(email, password) to signed
                            }
                        }
                    }
                    lockBusy = false
                    result.onSuccess { (key, signed) ->
                        if (signed != null) {
                            val who = signed.email.ifBlank { email }
                            app.store.networkEmail = who
                            account = who
                            netEmail = who
                            if (signed.refresh.isNotBlank()) app.store.saveNetworkRefresh(signed.refresh)
                            rememberAccountServer(signed.address, signed.fingerprint, signed.lan, signed.publicIp)
                            engine.command(JSONObject().put("cmd", "accessToken").put("token", signed.access).toString())
                            commitAutofill()
                        }
                        val salt = PinWrap.newSalt()
                        app.store.savePinSalt(salt)
                        app.store.pinWrap = PinWrap.wrap(key, pin, salt)
                        app.store.pinFails = 0
                        app.holdVaultKey(key)
                        changingPin = false
                        vaultRejected = false
                        app.markUnlocked()
                        if (enableBio && BioVault.available(this@MainActivity)) {
                            app.bioPrompt = true
                            BioVault.sealWithAuth(
                                this@MainActivity,
                                key,
                                onSealed = { wrap ->
                                    app.bioPrompt = false
                                    app.store.bioWrap = wrap
                                    app.store.biometricsOn = true
                                },
                                onCancel = { app.bioPrompt = false },
                                onError = {
                                    app.bioPrompt = false
                                    lockError = it
                                },
                            )
                        } else if (!enableBio) {
                            app.store.biometricsOn = false
                            app.store.bioWrap = ""
                            BioVault.forget()
                        }
                        syncPasswords(true)
                    }.onFailure {
                        lockError = it.message ?: "Could not save the PIN."
                    }
                }
            }
            fun reauth(password: String) {
                val email = netEmail.ifBlank { account }.trim()
                lockBusy = true
                lockError = ""
                scope.launch {
                    val result = withContext(Dispatchers.IO) {
                        runCatching {
                            val device = app.store.networkDeviceId.ifBlank {
                                UUID.randomUUID().toString().also { app.store.networkDeviceId = it }
                            }
                            val signed = CloudAuth.signIn(email, password, device, "Android")
                            signed to CloudAuth.vaultKey(email, password)
                        }
                    }
                    lockBusy = false
                    result.onSuccess { (signed, key) ->
                        commitAutofill()
                        val who = signed.email.ifBlank { email }
                        app.store.networkEmail = who
                        account = who
                        netEmail = who
                        if (signed.refresh.isNotBlank()) app.store.saveNetworkRefresh(signed.refresh)
                        rememberAccountServer(signed.address, signed.fingerprint, signed.lan, signed.publicIp)
                        engine.command(JSONObject().put("cmd", "accessToken").put("token", signed.access).toString())
                        app.store.clearPinMaterial()
                        BioVault.forget()
                        app.holdVaultKey(key)
                        app.store.pinFails = 0
                        vaultRejected = false
                        lockError = ""
                        changingPin = false
                        app.markUnlocked()
                    }.onFailure {
                        lockError = it.message ?: "Could not sign in."
                    }
                }
            }
            LaunchedEffect(account, temporarySession, uiUnlocked) {
                val persistent = account.isNotEmpty() && !temporarySession
                if (!persistent || uiUnlocked || vaultRejected || !app.store.hasPin()) return@LaunchedEffect
                if (app.store.pinFails >= AppLockPolicy.MAX_FAILS) return@LaunchedEffect
                if (!app.store.biometricsOn || app.store.bioWrap.isBlank()) return@LaunchedEffect
                tryBio()
            }
            PjTheme(theme.value) {
                val session = state.session
                val persistent = account.isNotEmpty() && !temporarySession
                val bioAvailable = BioVault.available(this@MainActivity)
                val gate = when {
                    !persistent -> null
                    vaultRejected || (app.store.hasPin() && app.store.pinFails >= AppLockPolicy.MAX_FAILS) -> "reauth"
                    !app.store.hasPin() -> "pin"
                    !uiUnlocked -> "unlock"
                    changingPin -> "change"
                    else -> null
                }
                if (gate == "pin" || gate == "change") {
                    SetPinScreen(
                        email = netEmail.ifBlank { account },
                        onEmail = { netEmail = it },
                        needPassword = app.vaultKey == null,
                        busy = lockBusy,
                        error = lockError,
                        biometricsAvailable = bioAvailable,
                        biometricsDefault = if (gate == "change") app.store.biometricsOn else bioAvailable,
                        title = if (gate == "change") "Change PIN" else "Set an app PIN",
                        detail = if (gate == "change") {
                            "Choose a new 4 to 8 digit PIN. Your account stays signed in."
                        } else {
                            "PJDesk stays signed in. Choose a PIN so opening the app asks for it."
                        },
                        onSave = { pin, password, enableBio -> finishPin(pin, password, enableBio) },
                        onCancel = if (gate == "change") {
                            { changingPin = false }
                        } else {
                            null
                        },
                    )
                } else if (gate == "unlock") {
                    UnlockScreen(
                        busy = lockBusy,
                        error = lockError,
                        biometrics = bioAvailable && app.store.biometricsOn && app.store.bioWrap.isNotBlank(),
                        rejectCount = pinRejects,
                        onSubmit = { submitPin(it) },
                        onBiometric = { tryBio() },
                    )
                } else if (gate == "reauth") {
                    ReauthScreen(
                        email = netEmail.ifBlank { account },
                        onEmail = { netEmail = it },
                        busy = lockBusy,
                        error = lockError,
                        onSubmit = { reauth(it) },
                    )
                } else if (session != null) {
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
                    LaunchedEffect(session.id) { engine.keyframe() }
                    SessionScreen(
                        session = session,
                        onGesture = engine::gesture,
                        onMouse = engine::mouse,
                        onText = engine::text,
                        onSpecial = engine::special,
                        onKey = engine::key,
                        onQuality = engine::quality,
                        onQualityCustom = engine::qualityCustom,
                        onPrivacy = engine::privacy,
                        onKeyboardMode = engine::keyboardMode,
                        onDisplay = { index -> engine.display(index, session.layout) },
                        onEnd = { engine.stopView("disconnected") },
                        remote = sessionRemote,
                        onRemote = { saveSessionRemote(it) },
                        forThisComputer = peerOnly,
                        onForThisComputer = { setPeerOnly(it) },
                        fileNames = fileNames,
                        onFileList = { engine.fileList() },
                        onSendFile = { sendFile.launch("*/*") },
                        onRestart = { engine.restart() },
                        idleWarn = sessionIdleWarn,
                        onStay = { sessionIdleWarn = false },
                        onActivity = {},
                        onClipboard = {
                            val text = clipboardText()
                            if (text.isBlank()) {
                                scope.launch { snackbar.showSnackbar("Clipboard is empty") }
                            } else {
                                engine.paste(text)
                            }
                        },
                        decoder = sessionDecoder,
                    )
                } else if (netScan) {
                    QrScan(
                        onResult = {
                            netBlob = it
                            netScan = false
                        },
                        onClose = { netScan = false },
                    )
                } else if (account.isEmpty() && (state.server.isEmpty() || signingIn)) {
                    SignInScreen(
                        email = netEmail,
                        password = netPass,
                        code = netCode,
                        useCode = netUseCode,
                        useKey = netUseKey,
                        blob = netBlob,
                        keyPass = netKeyPass,
                        busy = netBusy,
                        error = netError,
                        onEmail = { if (!notePaste(it)) netEmail = it },
                        onPassword = { if (!notePaste(it)) netPass = it },
                        onCode = { if (!notePaste(it)) netCode = it },
                        onBlob = {
                            if (notePaste(it)) {
                                netBlob = NetKey.blobIn(it) ?: it
                                netUseKey = true
                            } else netBlob = it
                        },
                        onKeyPass = { if (!notePaste(it)) netKeyPass = it },
                        onUseCode = { netUseCode = it; if (it) netUseKey = false },
                        onUseKey = { netUseKey = it; if (it) netUseCode = false },
                        onScan = {
                            val granted = ContextCompat.checkSelfPermission(this@MainActivity, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
                            if (granted) netScan = true else cameraPermission.launch(Manifest.permission.CAMERA)
                        },
                        accountForKey = netError.contains("specific account"),
                        importOffer = if (importFromPaste) importOffer else null,
                        importPass = importPass,
                        onImportPass = { if (!notePaste(it)) importPass = it },
                        onAcceptImport = {
                            val blob = importOffer ?: return@SignInScreen
                            val bound = netError.contains("specific account")
                            applyImported(blob, importPass, bound)
                        },
                        onDismissImport = { importOffer = null; importPass = "" },
                        temporary = signOutWhenDone,
                        onTemporary = { signOutWhenDone = it },
                        onEnrollOnly = {
                            val device = app.store.networkDeviceId.ifBlank {
                                UUID.randomUUID().toString().also { app.store.networkDeviceId = it }
                            }
                            netBusy = true
                            netError = ""
                            val email = netEmail
                            val pass = netPass
                            scope.launch {
                                val result = withContext(Dispatchers.IO) {
                                    runCatching { CloudAuth.signIn(email, pass, device, "Android") }
                                }
                                result.onSuccess { session ->
                                    applySignedIn(session, email, null, false, true)
                                }.onFailure { err ->
                                    netBusy = false
                                    netError = err.message ?: "Could not sign in."
                                }
                            }
                        },
                        offerCurrent = netOffer,
                        onUseCurrent = {
                            val token = netLogin
                            val address = state.server
                            val fingerprint = state.fingerprint
                            val device = app.store.networkDeviceId.ifBlank {
                                UUID.randomUUID().toString().also { app.store.networkDeviceId = it }
                            }
                            if (token.isBlank() || address.isBlank() || fingerprint.isBlank()) return@SignInScreen
                            netBusy = true
                            netError = ""
                            scope.launch {
                                val result = withContext(Dispatchers.IO) {
                                    runCatching { CloudAuth.claimCurrentServer(token, address, fingerprint, device, "Android") }
                                }
                                result.onSuccess { session ->
                                    applySignedIn(session, netEmail, null, signOutWhenDone, false, accountPassword = netPass)
                                }.onFailure {
                                    netBusy = false
                                    netError = it.message ?: "Could not sign in."
                                }
                            }
                        },
                        onSubmit = {
                            val device = app.store.networkDeviceId.ifBlank {
                                UUID.randomUUID().toString().also { app.store.networkDeviceId = it }
                            }
                            val bound = netError.contains("specific account")
                            netBusy = true
                            netError = ""
                            val email = netEmail
                            val pass = netPass
                            val code = netCode
                            val blob = netBlob
                            val keyPass = netKeyPass
                            val useCode = netUseCode
                            val useKey = netUseKey
                            scope.launch {
                                val result = withContext(Dispatchers.IO) {
                                    runCatching {
                                        when {
                                            useKey -> CloudAuth.redeem(
                                                blob,
                                                keyPass,
                                                device,
                                                "Android",
                                                if (bound) email else "",
                                                if (bound) pass else "",
                                            )
                                            useCode -> CloudAuth.enroll(code, device, "Android")
                                            else -> CloudAuth.signIn(email, pass, device, "Android")
                                        }
                                    }
                                }
                                result.onSuccess { session ->
                                    val accountPassword = when {
                                        useKey -> if (bound) pass else ""
                                        useCode -> ""
                                        else -> pass
                                    }
                                    applySignedIn(session, email, if (useKey) blob else null, signOutWhenDone, false, accountPassword = accountPassword)
                                }.onFailure { err ->
                                    netBusy = false
                                    val cloud = err as? CloudAuthException
                                    netOffer = cloud != null && cloud.code == "no_server" && CloudAuth.offerCurrentServer(
                                        if (cloud.role.isBlank()) "owner" else cloud.role,
                                        state.server,
                                        state.fingerprint,
                                    )
                                    netLogin = if (netOffer) cloud?.loginToken.orEmpty() else ""
                                    netError = err.message ?: "Could not sign in."
                                }
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
                        computerName = app.store.computerName,
                        deviceLabel = engine.deviceLabel(),
                        onComputerName = { engine.setDisplayName(it) },
                        onDone = {
                            app.store.onboarded = true
                            showOnboarding = false
                        },
                    )
                } else {
                    Scaffold(
                        snackbarHost = { SnackbarHost(snackbar) },
                        containerColor = androidx.compose.material3.MaterialTheme.colorScheme.background,
                        topBar = {
                            TopAppBar(
                                title = {
                                    Text(
                                        when (tab) {
                                            0 -> "PJDesk"
                                            1 -> "Saved computers"
                                            else -> "Settings"
                                        },
                                    )
                                },
                                colors = TopAppBarDefaults.topAppBarColors(
                                    containerColor = androidx.compose.material3.MaterialTheme.colorScheme.background,
                                    titleContentColor = androidx.compose.material3.MaterialTheme.colorScheme.onBackground,
                                ),
                            )
                        },
                        bottomBar = {
                            NavigationBar(containerColor = androidx.compose.material3.MaterialTheme.colorScheme.surface) {
                                NavigationBarItem(
                                    selected = tab == 0,
                                    onClick = { tab = 0 },
                                    icon = { Icon(Icons.Filled.Home, contentDescription = null) },
                                    label = { Text("Home") },
                                    alwaysShowLabel = true,
                                )
                                NavigationBarItem(
                                    selected = tab == 1,
                                    onClick = { tab = 1 },
                                    icon = { Icon(Icons.Filled.List, contentDescription = null) },
                                    label = { Text("Saved") },
                                    alwaysShowLabel = true,
                                )
                                NavigationBarItem(
                                    selected = tab == 2,
                                    onClick = { tab = 2 },
                                    icon = { Icon(Icons.Filled.Settings, contentDescription = null) },
                                    label = { Text("Settings") },
                                    alwaysShowLabel = true,
                                )
                            }
                        },
                    ) { padding ->
                        androidx.compose.foundation.layout.Box(Modifier.padding(padding)) {
                            when (tab) {
                                0 -> HomeScreen(
                                    state = state,
                                    connectId = connectId,
                                    connectName = connectComputerName(saved, connectId),
                                    onConnectId = { connectId = it },
                                    onConnect = { beginConnect(connectId) },
                                    onCopyId = { copy(state.idFormatted) },
                                    onCopyPassword = { copy(state.password) },
                                    onRegenerate = engine::regenerate,
                                    onStop = { HostService.stop(this@MainActivity) },
                                    showPermanent = showPermanent,
                                    onShowPermanent = { showPermanent = it },
                                    onSavePermanent = { password, confirm -> savePermanent(password, confirm) },
                                    onClearPermanent = { removePermanent() },
                                    controlEnabled = controlEnabled,
                                    controlBound = controlBound,
                                    onOpenControl = { PjAccessibilityService.openSettings(this@MainActivity) },
                                    onUnattended = { engine.setApproval(if (it) "unattended" else "prompt") },
                                    onSignIn = if (account.isEmpty() && state.server.isNotEmpty()) {
                                        { signingIn = true }
                                    } else {
                                        null
                                    },
                                    accountEmail = account,
                                    temporary = temporarySession,
                                    recent = saved.filter { recentIds.contains(it.id) }.take(3),
                                    onRecent = { beginConnect(it) },
                                )
                                1 -> SavedScreen(
                                    signedIn = account.isNotEmpty(),
                                    computers = saved,
                                    recentIds = recentIds,
                                    query = query,
                                    onQuery = { query = it },
                                    status = if (
                                        connecting && !promptBeforeConnect && !state.askPassword &&
                                        (
                                            state.status.startsWith("Connecting") ||
                                                state.status.startsWith("Getting your PJDesk server") ||
                                                state.status.startsWith("Starting PJDesk")
                                            )
                                    ) {
                                        state.status
                                    } else {
                                        ""
                                    },
                                    sort = savedSort,
                                    filter = savedFilter,
                                    onSort = { chosen -> scope.launch { listPrefs.setSort(chosen) } },
                                    onFilter = { chosen -> scope.launch { listPrefs.setFilter(chosen) } },
                                    onConnect = { beginConnect(it.id) },
                                    onForget = {
                                        app.store.forget(it.id)
                                        saved = app.store.saved()
                                        syncPasswords(true)
                                    },
                                    onRemovePassword = {
                                        app.store.stampPassword(it.id, "")
                                        saved = app.store.saved()
                                        syncPasswords(true)
                                    },
                                    selfId = state.id,
                                    onRename = { computer, nickname ->
                                        app.store.rename(computer.id, nickname)
                                        saved = app.store.saved()
                                        val refresh = app.store.networkRefresh()
                                        val device = app.store.networkDeviceId
                                        if (refresh.isNotBlank() && device.isNotBlank()) {
                                            scope.launch {
                                                withContext(Dispatchers.IO) {
                                                    runCatching {
                                                        refreshAccount(refresh, device, CloudAuth.computersJson(app.store.saved()))
                                                    }
                                                }
                                            }
                                        }
                                    },
                                )
                                else -> SettingsScreen(
                                    state = state,
                                    computerName = app.store.computerName,
                                    deviceLabel = engine.deviceLabel(),
                                    onComputerName = { engine.setDisplayName(it) },
                                    tempIdleMinutes = app.store.tempIdleMinutes,
                                    onTempIdleMinutes = { app.store.tempIdleMinutes = it },
                                    theme = theme.value,
                                    onTheme = {
                                        theme.value = it
                                        app.store.theme = it
                                    },
                                    onServer = engine::setServer,
                                    onResetFingerprint = engine::resetFingerprint,
                                    onSavePermanent = { password, confirm -> savePermanent(password, confirm) },
                                    onClearPermanent = { removePermanent() },
                                    controlEnabled = controlEnabled,
                                    controlBound = controlBound,
                                    onOpenControl = { PjAccessibilityService.openSettings(this@MainActivity) },
                                    onUnattended = { engine.setApproval(if (it) "unattended" else "prompt") },
                                    trustedAccounts = trustedAccounts,
                                    trustedComputers = trustedComputers,
                                    onAllowAccount = {
                                        engine.allowAccount(it)
                                        trustedAccounts = app.store.trustedAccounts()
                                    },
                                    onAllowComputer = {
                                        engine.allowComputer(it)
                                        trustedComputers = app.store.trustedComputers()
                                    },
                                    onForgetTrusted = { kind, value ->
                                        engine.forgetTrusted(kind, value)
                                        trustedAccounts = app.store.trustedAccounts()
                                        trustedComputers = app.store.trustedComputers()
                                    },
                                    onShareLogs = { AppLog.share(this@MainActivity) },
                                    onCheckUpdate = { engine.checkForUpdate() },
                                    signedIn = account.isNotEmpty() && !temporarySession,
                                    hasPin = app.store.hasPin(),
                                    biometricsOn = app.store.biometricsOn,
                                    biometricsAvailable = BioVault.available(this@MainActivity),
                                    autoLockSeconds = app.store.autoLockSeconds,
                                    onChangePin = { changingPin = true; lockError = "" },
                                    onToggleBiometrics = { enabled ->
                                        if (!enabled) {
                                            app.store.biometricsOn = false
                                            app.store.bioWrap = ""
                                            BioVault.forget()
                                        } else {
                                            val key = app.vaultKey
                                            if (key == null) {
                                                scope.launch { snackbar.showSnackbar("Unlock PJDesk before turning on biometrics.") }
                                            } else {
                                                app.bioPrompt = true
                                                BioVault.sealWithAuth(
                                                    this@MainActivity,
                                                    key,
                                                    onSealed = { wrap ->
                                                        app.bioPrompt = false
                                                        app.store.bioWrap = wrap
                                                        app.store.biometricsOn = true
                                                    },
                                                    onCancel = { app.bioPrompt = false },
                                                    onError = {
                                                        app.bioPrompt = false
                                                        scope.launch { snackbar.showSnackbar(it) }
                                                    },
                                                )
                                            }
                                        }
                                    },
                                    onAutoLock = { app.store.autoLockSeconds = it },
                                    remote = remote,
                                    onRemote = {
                                        remote = it
                                        app.store.saveRemoteGlobal(it)
                                    },
                                    engineDetail = readiness.detail,
                                    accountEmail = account,
                                    onSignOut = if (account.isEmpty()) {
                                        null
                                    } else {
                                        {
                                            if (temporarySession) {
                                                signOutTemporary()
                                                scope.launch { snackbar.showSnackbar("Signed out") }
                                            } else {
                                                app.store.networkEmail = ""
                                                app.store.saveNetworkRefresh("")
                                                app.lockUi()
                                                engine.command(JSONObject().put("cmd", "accessToken").put("token", "").toString())
                                                account = ""
                                            }
                                        }
                                    },
                                    onImportConfig = {
                                        val text = clipboardText()
                                        val blob = NetKey.blobIn(text)
                                        when {
                                            blob == null -> scope.launch {
                                                snackbar.showSnackbar(if (text.isBlank()) "Clipboard is empty" else "Invalid server configuration")
                                            }
                                            NetKey.kind(blob) == "embedded" -> applyImported(blob, "", false)
                                            NetKey.kind(blob) == "passphrase" -> {
                                                importOffer = blob
                                                importPass = ""
                                                importFromPaste = false
                                            }
                                            else -> scope.launch { snackbar.showSnackbar("Invalid server configuration") }
                                        }
                                    },
                                    onExportConfig = {
                                        val blob = app.store.networkKey
                                        if (blob.isBlank()) {
                                            scope.launch { snackbar.showSnackbar("There is no server config to export.") }
                                        } else {
                                            copy(blob)
                                            scope.launch { snackbar.showSnackbar("Export server configuration successfully") }
                                        }
                                    },
                                    importOffer = if (!importFromPaste) importOffer else null,
                                    importPass = importPass,
                                    onImportPass = { if (!notePaste(it)) importPass = it },
                                    onAcceptImport = {
                                        val blob = importOffer ?: return@SettingsScreen
                                        applyImported(blob, importPass, netError.contains("specific account"))
                                    },
                                    onDismissImport = { importOffer = null; importPass = "" },
                                )
                            }
                        }
                    }
                }
                if (state.hosting && !(controlEnabled && controlBound) && !controlDismissed) {
                    AlertDialog(
                        onDismissRequest = { controlDismissed = true },
                        title = { Text("Control not enabled on phone") },
                        text = {
                            Text(
                                "A computer can see this phone, but it cannot tap, type, or use Back, Home, and the other buttons until PJDesk is turned on in Accessibility. ${us.spader.pjdesk.core.controlStatus(controlEnabled, controlBound)}",
                            )
                        },
                        confirmButton = {
                            TextButton(onClick = {
                                controlDismissed = true
                                PjAccessibilityService.openSettings(this@MainActivity)
                            }) { Text("Open Accessibility") }
                        },
                        dismissButton = {
                            TextButton(onClick = { controlDismissed = true }) { Text("Not now") }
                        },
                    )
                }
                if (tempWarn && temporarySession) {
                    AlertDialog(
                        onDismissRequest = {},
                        title = { Text("Still there?") },
                        text = { Text("Signing out in $tempLeft seconds because PJDesk is idle.") },
                        confirmButton = {
                            TextButton(onClick = {
                                lastActivity = android.os.SystemClock.elapsedRealtime()
                                tempWarn = false
                            }) { Text("Stay signed in") }
                        },
                        dismissButton = {
                            TextButton(onClick = {
                                signOutTemporary()
                                scope.launch { snackbar.showSnackbar("Signed out") }
                            }) { Text("Sign out") }
                        },
                    )
                }
                if (askChoice && !promptBeforeConnect && !state.askPassword) {
                    val titleName = connectComputerName(saved, connectId).ifBlank { "this computer" }
                    AlertDialog(
                        onDismissRequest = { askChoice = false },
                        title = { Text("Connect to $titleName") },
                        text = { Text("Enter the password, or ask the person at $titleName to accept.") },
                        confirmButton = {
                            TextButton(onClick = {
                                askChoice = false
                                promptBeforeConnect = true
                                askedPassword = ""
                            }) { Text("Enter password") }
                        },
                        dismissButton = {
                            Row {
                                TextButton(onClick = {
                                    askChoice = false
                                    askWaiting = true
                                    launchConnect(connectId, titleName, "", askPerson = true)
                                }) { Text("Ask the person at $titleName") }
                                TextButton(onClick = { askChoice = false }) { Text("Cancel") }
                            }
                        },
                    )
                }
                if (askWaiting && state.session == null && connectError == null && !state.askPassword && !promptBeforeConnect) {
                    val titleName = connectComputerName(saved, connectId).ifBlank { "that computer" }
                    AlertDialog(
                        onDismissRequest = {},
                        title = { Text("Waiting for $titleName to accept…") },
                        text = { Text("They can allow or deny this connection. This stops after 60 seconds.") },
                        confirmButton = {
                            TextButton(onClick = {
                                askWaiting = false
                                connecting = false
                                engine.stopView("cancelled")
                            }) { Text("Cancel") }
                        },
                    )
                }
                if (promptBeforeConnect || state.askPassword) {
                    val titleName = state.askName.ifBlank {
                        connectComputerName(saved, connectId).ifBlank { "this computer" }
                    }
                    AlertDialog(
                        onDismissRequest = {
                            if (state.askPassword) engine.cancelPassword()
                            promptBeforeConnect = false
                            askedPassword = ""
                            connecting = false
                            if (state.status.startsWith("Connecting")) engine.showStatus("Ready")
                        },
                        title = { Text("Password for $titleName") },
                        text = {
                            Column {
                                if (state.askNote.isNotBlank()) {
                                    Text(state.askNote)
                                } else if (state.askRejected) {
                                    Text("Incorrect password. Enter the current one for this computer.")
                                }
                                SecretField(
                                    value = askedPassword,
                                    onValueChange = { askedPassword = it },
                                    label = "Computer password",
                                )
                                Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                                    Checkbox(checked = savePassword, onCheckedChange = { savePassword = it })
                                    Text("Save password for this computer")
                                }
                            }
                        },
                        confirmButton = {
                            TextButton(onClick = {
                                val typed = askedPassword
                                if (typed.isBlank()) return@TextButton
                                val id = connectId.filter { it.isDigit() }
                                if (savePassword) {
                                    app.store.stampPassword(id, typed)
                                    if (temporarySession) app.store.noteTemporaryPassword(id)
                                    saved = app.store.saved()
                                    syncPasswords(true)
                                }
                                askedPassword = ""
                                if (state.askPassword) {
                                    connecting = true
                                    engine.showStatus(connectStatusLabel(connectComputerName(saved, id), id))
                                    engine.submitPassword(typed)
                                } else {
                                    launchConnect(id, connectComputerName(saved, id), typed)
                                }
                            }) { Text("Connect") }
                        },
                        dismissButton = {
                            Row {
                                TextButton(onClick = {
                                    val id = connectId.filter { it.isDigit() }
                                    askedPassword = ""
                                    if (state.askPassword) {
                                        engine.askPerson()
                                    } else {
                                        launchConnect(id, connectComputerName(saved, id), "", askPerson = true)
                                    }
                                    askWaiting = true
                                    promptBeforeConnect = false
                                }) { Text("Ask to connect instead") }
                                TextButton(onClick = {
                                    if (state.askPassword) engine.cancelPassword()
                                    promptBeforeConnect = false
                                    askedPassword = ""
                                    connecting = false
                                    if (state.status.startsWith("Connecting")) engine.showStatus("Ready")
                                }) { Text("Cancel") }
                            }
                        },
                    )
                }
                val failure = connectError
                if (failure != null) {
                    AlertDialog(
                        onDismissRequest = {
                            connectError = null
                            offlineRetry = null
                        },
                        title = { Text("Couldn't connect") },
                        text = { Text(failure) },
                        confirmButton = {
                            val retry = offlineRetry
                            if (retry != null) {
                                TextButton(onClick = {
                                    offlineRetry = null
                                    connectError = null
                                    beginConnect(retry.id, ignoreOffline = true)
                                }) { Text("Try anyway") }
                            } else {
                                TextButton(onClick = { connectError = null }) { Text("OK") }
                            }
                        },
                        dismissButton = if (offlineRetry != null) {
                            {
                                TextButton(onClick = {
                                    offlineRetry = null
                                    connectError = null
                                }) { Text("Cancel") }
                            }
                        } else {
                            null
                        },
                    )
                }
                val incoming = state.incoming
                if (incoming != null && !incoming.view && !pendingAccept) {
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
        if (isFinishing && !isChangingConfigurations && app.engine.state.value.session != null) {
            app.engine.stopView("closed")
        }
        if (isFinishing) {
            decoder?.stop()
            if (tempFlag.get()) {
                tempFlag.set(false)
                app.store.endTemporaryVisit()
            }
        }
        super.onDestroy()
    }

    private fun refreshAccount(
        refresh: String,
        device: String,
        computers: org.json.JSONArray?,
    ): us.spader.pjdesk.core.Session {
        val androidId = Settings.Secure.getString(contentResolver, Settings.Secure.ANDROID_ID).orEmpty()
        return us.spader.pjdesk.core.CloudAuth.refresh(
            refresh,
            device,
            computers,
            pjdeskId = app.engine.state.value.id,
            hardwareId = us.spader.pjdesk.core.CloudAuth.hardwareKey(androidId),
            hostname = app.engine.deviceLabel(),
        )
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
        HostService.consentInProgress = true
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

/** The computer's nickname or hostname, for the connect password label. */
internal fun connectComputerName(saved: List<SavedComputer>, connectId: String): String {
    val id = connectId.filter { it.isDigit() }
    if (id.isEmpty()) return ""
    val computer = saved.firstOrNull { it.id == id } ?: return ""
    val nick = computer.nickname()
    return when {
        nick.isNotEmpty() -> nick
        computer.hostname.isNotBlank() -> computer.hostname
        else -> ""
    }
}
