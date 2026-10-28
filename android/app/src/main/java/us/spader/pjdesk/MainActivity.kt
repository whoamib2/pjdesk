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
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.OutlinedTextField
import androidx.compose.ui.Alignment
import androidx.compose.material3.Scaffold
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import androidx.compose.runtime.rememberCoroutineScope
import org.json.JSONObject
import us.spader.pjdesk.core.AccountPin
import us.spader.pjdesk.core.AppLockPolicy
import us.spader.pjdesk.core.BioVault
import us.spader.pjdesk.core.CloudAuth
import us.spader.pjdesk.core.CloudAuthException
import us.spader.pjdesk.core.CloudGate
import us.spader.pjdesk.core.EnginePhase
import us.spader.pjdesk.core.NetKey
import us.spader.pjdesk.core.PasswordSync
import us.spader.pjdesk.core.PinWrap
import us.spader.pjdesk.core.VaultKeyRejected
import us.spader.pjdesk.core.connectFailureText
import us.spader.pjdesk.core.connectStatusLabel
import us.spader.pjdesk.core.shouldRetryNotOnline
import us.spader.pjdesk.ui.ReauthScreen
import us.spader.pjdesk.ui.SecretField
import us.spader.pjdesk.ui.SetPinScreen
import us.spader.pjdesk.ui.SignInScreen
import us.spader.pjdesk.ui.UnlockScreen
import java.util.UUID
import us.spader.pjdesk.core.LanListen
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.SavedFilter
import us.spader.pjdesk.core.SavedSort
import us.spader.pjdesk.core.SavedView
import us.spader.pjdesk.core.applyLan
import us.spader.pjdesk.core.discoveredComputers
import us.spader.pjdesk.data.SavedListPrefs
import us.spader.pjdesk.core.nickname
import us.spader.pjdesk.a11y.PjAccessibilityService
import us.spader.pjdesk.core.permanentIssue
import us.spader.pjdesk.service.RemoteControl
import us.spader.pjdesk.media.ScreenDecoder
import us.spader.pjdesk.service.HostService
import us.spader.pjdesk.ui.HomeScreen
import us.spader.pjdesk.ui.OnboardingScreen
import us.spader.pjdesk.ui.PjBottomBar
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
                HostService.onConsentRestart = {
                    runOnUiThread {
                        HostService.onConsentRestart = null
                        HostService.onCapture = null
                    }
                }
                HostService.onCapture = { ok, reason ->
                    runOnUiThread {
                        HostService.onCapture = null
                        HostService.onConsentRestart = null
                        pendingAccept = false
                        HostService.acceptOnSuccess = false
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
                HostService.acceptOnSuccess = true
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
            HostService.acceptOnSuccess = false
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
        AppLog.i("main activity created task=$taskId")
        if (app.store.temporaryActive() && !tempFlag.get()) app.store.endTemporaryVisit()
        if (app.store.accountServer.isNotBlank()) {
            app.engine.applyAccountServer(app.store.accountServer, app.store.accountFingerprint, quiet = true, standby = app.store.accountStandby)
        }
        CloudGate.exchange = exchange@{ body ->
            if (android.os.Looper.myLooper() == android.os.Looper.getMainLooper()) return@exchange null
            val gen = CloudGate.accountGen.get()
            val want = body.optString("op")
            app.engine.command(JSONObject(body.toString()).put("cmd", "accountOp").toString())
            val deadline = System.currentTimeMillis() + 45_000
            while (System.currentTimeMillis() < deadline) {
                if (CloudGate.accountGen.get() != gen) {
                    val parsed = runCatching { JSONObject(CloudGate.accountJson) }.getOrNull()
                    val got = parsed?.optString("op").orEmpty()
                    if (want.isEmpty() || got.isEmpty() || got == want) return@exchange parsed
                }
                Thread.sleep(50)
            }
            null
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
            val savedView by listPrefs.view.collectAsStateWithLifecycle(SavedView.List)
            val snackbar = remember { SnackbarHostState() }
            var tab by remember { mutableIntStateOf(0) }
            var connectId by remember { mutableStateOf("") }
            var connectPassword by remember { mutableStateOf("") }
            var connectAsk by remember { mutableStateOf(false) }
            var notOnlineTries by remember { mutableIntStateOf(0) }
            var query by remember { mutableStateOf("") }
            var saved by remember { mutableStateOf(app.store.saved()) }
            var recentIds by remember { mutableStateOf(app.store.recentIds()) }
            var discovered by remember { mutableStateOf<List<SavedComputer>>(emptyList()) }
            val savedNow = rememberUpdatedState(saved)
            val selfNow = rememberUpdatedState(state.id)
            DisposableEffect(Unit) {
                val listen = LanListen(applicationContext) { sights ->
                    discovered = discoveredComputers(sights, savedNow.value, selfNow.value)
                }
                listen.start()
                onDispose { listen.stop() }
            }
            var trustedAccounts by remember { mutableStateOf(app.store.trustedAccounts()) }
            var trustedComputers by remember { mutableStateOf(app.store.trustedComputers()) }
            var showOnboarding by remember { mutableStateOf(!app.store.onboarded) }
            var account by remember { mutableStateOf(app.store.networkEmail) }
            var cloudAccess by remember { mutableStateOf("") }
            var inviteCode by remember { mutableStateOf("") }
            var inviteId by remember { mutableStateOf("") }
            var inviteExpires by remember { mutableLongStateOf(0L) }
            var inviteBusy by remember { mutableStateOf(false) }
            var inviteError by remember { mutableStateOf("") }
            var inviteListed by remember { mutableStateOf(false) }
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
            var signInPauseUntil by remember { mutableLongStateOf(0L) }
            LaunchedEffect(signInPauseUntil) {
                if (signInPauseUntil == 0L) return@LaunchedEffect
                while (true) {
                    val leftMs = signInPauseUntil - android.os.SystemClock.elapsedRealtime()
                    if (leftMs <= 0L) {
                        signInPauseUntil = 0L
                        if (netError.startsWith("Too many sign-in attempts")) netError = ""
                        break
                    }
                    netError = us.spader.pjdesk.core.signInPauseText((leftMs + 999) / 1000)
                    kotlinx.coroutines.delay(200)
                }
            }
            var signingIn by remember { mutableStateOf(false) }
            var netOffer by remember { mutableStateOf(false) }
            var netLogin by remember { mutableStateOf("") }
            var importOffer by remember { mutableStateOf<String?>(null) }
            var importPass by remember { mutableStateOf("") }
            var importFromPaste by remember { mutableStateOf(true) }
            var savePassword by remember { mutableStateOf(false) }
            var signOutWhenDone by remember { mutableStateOf(false) }
            var changingPin by remember { mutableStateOf(false) }
            var forgotPin by remember { mutableStateOf(false) }
            var visitWithoutPassword by remember { mutableStateOf(false) }
            var pinPending by remember {
                mutableStateOf(
                    !app.store.joinedWithoutPassword
                        && app.store.appLockSealed.isNotBlank() && app.store.appLockUpdated > 0L
                        && !app.store.hasAccountPin() && !app.store.hasPin(),
                )
            }
            var pinFormError by remember { mutableStateOf("") }
            var confirmUnattended by remember { mutableStateOf(false) }
            var unattendedPin by remember { mutableStateOf("") }
            var pinRejects by remember { mutableIntStateOf(0) }
            var pinEpoch by remember { mutableIntStateOf(0) }
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
                visitWithoutPassword = false
                memoryRefresh = ""
                app.store.endTemporaryVisit()
                app.clearVaultKey()
                cloudAccess = ""
                inviteCode = ""
                inviteId = ""
                inviteExpires = 0
                inviteListed = false
                engine.command(org.json.JSONObject().put("cmd", "accessToken").put("token", "").toString())
                engine.noteAccountSession("")
                account = ""
                saved = app.store.saved()
                recentIds = app.store.recentIds()
                tempWarn = false
                enrolledFor = ""
                accountComputers = emptyList()
            }
            fun signOutAndClearSaved() {
                tempFlag.set(false)
                temporarySession = false
                visitWithoutPassword = false
                memoryRefresh = ""
                if (app.store.temporaryActive()) {
                    app.store.endTemporaryVisit()
                } else {
                    app.store.clearLocalSaved()
                    app.store.networkEmail = ""
                    app.store.joinedWithoutPassword = false
                    app.store.saveNetworkRefresh("")
                }
                app.store.savedSyncChoice = "no"
                app.clearVaultKey()
                cloudAccess = ""
                inviteCode = ""
                inviteId = ""
                inviteExpires = 0
                inviteListed = false
                engine.command(org.json.JSONObject().put("cmd", "accessToken").put("token", "").toString())
                engine.noteAccountSession("")
                account = ""
                saved = app.store.saved()
                recentIds = app.store.recentIds()
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
                if (!app.store.syncsSavedComputers()) return
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
                engine.setUnattendedAccess(false)
                publishPermanent(state.id)
                syncPasswords(true)
            }
            fun pinMatches(pin: String): Boolean {
                val salt = app.store.pinSaltBytes() ?: return false
                return PinWrap.unwrap(app.store.pinWrap, pin, salt) != null
            }
            fun requestUnattended(on: Boolean) {
                if (!on) {
                    engine.setUnattendedAccess(false)
                    return
                }
                when {
                    !state.hasPermanent -> scope.launch { snackbar.showSnackbar("Save a permanent password first.") }
                    !PjAccessibilityService.enabled(this@MainActivity) ->
                        scope.launch { snackbar.showSnackbar("Turn on control in Accessibility first.") }
                    !app.store.hasPin() && !BioVault.available(this@MainActivity) ->
                        scope.launch { snackbar.showSnackbar("Set a PIN in App lock before turning this on.") }
                    else -> {
                        if (app.store.hasPin()) {
                            unattendedPin = ""
                            confirmUnattended = true
                        }
                        if (BioVault.available(this@MainActivity)) {
                            app.bioPrompt = true
                            BioVault.confirm(
                                this@MainActivity,
                                "Turn on unattended access",
                                "Confirm with your fingerprint or face.",
                                if (app.store.hasPin()) "Use PIN" else "Cancel",
                                onSuccess = {
                                    app.bioPrompt = false
                                    confirmUnattended = false
                                    engine.setUnattendedAccess(true)
                                },
                                onCancel = { app.bioPrompt = false },
                                onError = {
                                    app.bioPrompt = false
                                    scope.launch { snackbar.showSnackbar(it) }
                                },
                            )
                        }
                    }
                }
            }
            fun rememberAccountServer(
                address: String,
                fingerprint: String,
                lan: List<String>,
                publicIp: String,
                standby: String = "",
                routes: String = "",
                standbyLan: List<String> = emptyList(),
                standbyPublicIp: String = "",
            ) {
                if (address.isBlank()) return
                app.store.accountServer = address
                app.store.accountFingerprint = fingerprint
                app.store.accountLan = lan.joinToString(",")
                app.store.accountPublicIp = publicIp
                if (standby.isNotBlank()) app.store.accountStandby = standby
                engine.applyAccountServer(
                    address,
                    fingerprint,
                    quiet = true,
                    standby = app.store.accountStandby,
                    routes = routes,
                    standbyLan = standbyLan.joinToString(","),
                    standbyPublicIp = standbyPublicIp,
                )
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
            fun noteSignInPause(err: Throwable): Boolean {
                val cloud = err as? us.spader.pjdesk.core.CloudAuthException
                val secs = us.spader.pjdesk.core.signInPauseSeconds(
                    cloud?.status ?: 0,
                    err.message.orEmpty(),
                    cloud?.retryAfter,
                ) ?: return false
                signInPauseUntil = android.os.SystemClock.elapsedRealtime() + secs * 1000
                netError = us.spader.pjdesk.core.signInPauseText(secs)
                return true
            }
            fun loadMySavedComputers() {
                if (state.incomingOnly) return
                app.store.savedSyncChoice = "yes"
                app.store.localSavedCleared = false
                val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                val device = app.store.networkDeviceId
                if (refresh.isBlank() || device.isBlank()) return
                scope.launch {
                    val session = withContext(Dispatchers.IO) {
                        runCatching { refreshAccount(refresh, device, null) }.getOrNull()
                    } ?: return@launch
                    app.store.mergeCloud(session.computers, preferAccount = true)
                    saved = app.store.saved()
                    if (session.access.isNotBlank()) {
                        cloudAccess = session.access
                        engine.presentAccess(session.access, session.refresh, session.deviceId)
                    }
                    rememberAccountServer(session.address, session.fingerprint, session.lan, session.publicIp, session.standbyAddress, session.routes, session.standbyLan, session.standbyPublicIp)
                    syncPasswords(false)
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
                withoutPassword: Boolean = false,
            ) {
                val pjdeskId = state.id.filter { it.isDigit() }
                val syncing = app.store.syncsSavedComputers()
                val uploadLocal = syncing && !temporaryNow && !enrollOnlyNow
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
                        signInPauseUntil = 0L
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
                        if (!syncing) app.store.savedSyncChoice = "no"
                        if (temporaryNow) {
                            app.store.beginTemporaryVisit()
                            tempFlag.set(true)
                            temporarySession = true
                            memoryRefresh = enrolled.refresh
                            lastActivity = android.os.SystemClock.elapsedRealtime()
                            if (syncing) {
                                app.store.allowSavedMerge()
                                app.store.mergeCloud(enrolled.computers, preferAccount = true)
                            }
                        } else {
                            if (syncing) {
                                val first = app.store.networkEmail.isBlank()
                                app.store.allowSavedMerge()
                                app.store.mergeCloud(enrolled.computers, preferAccount = first)
                            }
                            app.store.networkEmail = who
                            app.store.saveNetworkRefresh(enrolled.refresh)
                            if (!keyBlob.isNullOrBlank()) {
                                us.spader.pjdesk.core.NetKey.blobIn(keyBlob)?.let { app.store.networkKey = it }
                            }
                            app.markUnlocked()
                        }
                        if (withoutPassword) {
                            if (temporaryNow) visitWithoutPassword = true else app.store.joinedWithoutPassword = true
                            pinPending = false
                            app.markUnlocked()
                        } else if (!temporaryNow) {
                            app.store.joinedWithoutPassword = false
                            visitWithoutPassword = false
                        }
                        accountComputers = enrolled.computers
                        enrolledFor = if (pjdeskId.length == 9) pjdeskId else ""
                        rememberAccountServer(enrolled.address, enrolled.fingerprint, enrolled.lan, enrolled.publicIp, enrolled.standbyAddress, enrolled.routes, enrolled.standbyLan, enrolled.standbyPublicIp)
                        cloudAccess = enrolled.access
                        engine.presentAccess(enrolled.access, enrolled.refresh, enrolled.deviceId)
                        account = who
                        signingIn = false
                        saved = app.store.saved()
                        if (!enrollOnlyNow) holdDerivedKey(who, accountPassword, upload = syncing && !temporaryNow)
                        val connected = enrolled.email.ifBlank { enrolled.address }.ifBlank { "this network" }
                        if (withoutPassword && !enrollOnlyNow) {
                            scope.launch { snackbar.showSnackbar("You are now connected to $connected.") }
                        } else if (temporaryNow) {
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
                        if (!noteSignInPause(err)) netError = us.spader.pjdesk.core.signInPageError(err.message ?: "Could not sign in.")
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
            fun launchConnect(
                id: String,
                name: String,
                password: String,
                askPerson: Boolean = false,
                resetNotOnline: Boolean = true,
            ) {
                if (state.incomingOnly) {
                    connecting = false
                    connectError = "Incoming only is on. This computer can't connect to others."
                    return
                }
                val now = android.os.SystemClock.elapsedRealtime()
                if (connecting && now - lastConnectMs < 700) {
                    AppLog.i("Connect ignored, already starting $id")
                    return
                }
                connectPassword = password
                connectAsk = askPerson
                if (resetNotOnline) notOnlineTries = 0
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
            val liveSaved = applyLan(saved, discovered.map { it.id.filter { ch -> ch.isDigit() } }.toSet())
            var presenceJob by remember { mutableStateOf<Job?>(null) }
            var presenceBusy by remember { mutableStateOf(false) }
            fun pullPresence() {
                if (presenceJob?.isActive == true) return
                val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                val device = app.store.networkDeviceId
                val signed = account.isNotBlank() && refresh.isNotBlank() && device.isNotBlank() && !temporarySession
                presenceJob = scope.launch {
                    presenceBusy = true
                    try {
                        if (!signed) return@launch
                        val known = state.server.ifBlank { app.store.accountServer }
                        if (!CloudGate.serverUp && known.isNotBlank()) return@launch
                        if (!CloudGate.serverUp && !CloudGate.allowCloud()) return@launch
                        val session = withContext(Dispatchers.IO) {
                            runCatching { refreshAccount(refresh, device, null) }
                                .onFailure { AppLog.e("presence refresh failed: ${it.message}") }
                                .getOrNull()
                        } ?: return@launch
                        if (session.access.isNotBlank()) {
                            cloudAccess = session.access
                            engine.presentAccess(session.access, session.refresh, session.deviceId)
                        }
                        rememberAccountServer(session.address, session.fingerprint, session.lan, session.publicIp, session.standbyAddress, session.routes, session.standbyLan, session.standbyPublicIp)
                        if (app.store.syncsSavedComputers() && !state.incomingOnly) {
                            app.store.mergeCloud(session.computers)
                            saved = app.store.saved()
                        }
                    } finally {
                        presenceBusy = false
                    }
                }
            }
            LaunchedEffect(tab, account, temporarySession) {
                if (account.isBlank() || temporarySession) return@LaunchedEffect
                if (tab != 0 && tab != 1) return@LaunchedEffect
                while (true) {
                    pullPresence()
                    delay(20_000)
                }
            }
            fun beginConnect(rawId: String, ignoreOffline: Boolean = false) {
                if (state.incomingOnly) {
                    connecting = false
                    connectError = "Incoming only is on. This computer can't connect to others."
                    return
                }
                val digits = rawId.filter { it.isDigit() }
                val computer = liveSaved.firstOrNull { it.id == digits }
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
                        val computers = if (temporarySession || !app.store.syncsSavedComputers()) null else CloudAuth.computersJson(app.store.saved())
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
                            rememberAccountServer(fetched.address, fetched.fingerprint, fetched.lan, fetched.publicIp, fetched.standbyAddress, fetched.routes, fetched.standbyLan, fetched.standbyPublicIp)
                            cloudAccess = fetched.access
                            engine.presentAccess(fetched.access, fetched.refresh, fetched.deviceId)
                            beginConnect(rawId, ignoreOffline)
                        }
                        return
                    }
                    connecting = false
                    connectError = "This phone has no server address yet. Sign in again, or set the server in Settings."
                    AppLog.e(connectError.orEmpty())
                    return
                }
                if (
                    computer != null &&
                    knownOffline &&
                    !ignoreOffline &&
                    us.spader.pjdesk.core.blockOfflineConnect(computer.online, computer.lastSeen, System.currentTimeMillis())
                ) {
                    connecting = false
                    offlineRetry = computer
                    val who = name.ifBlank { computer.hostname.ifBlank { null } ?: "That computer" }
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
                        applySignedIn(session, email, blob, signOutWhenDone, false, "Import server configuration successfully", if (bound) pass else "", withoutPassword = true)
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
            var hostChat by remember { mutableStateOf(false) }
            LaunchedEffect(state.supportCode, state.id) {
                if (isFinishing) return@LaunchedEffect
                val code = state.supportCode
                val id = state.id
                if (code.length != 8 || id.length != 9) return@LaunchedEffect
                while (true) {
                    withContext(Dispatchers.IO) {
                        runCatching { us.spader.pjdesk.core.CloudAuth.offerSupport(code, id) }
                    }
                    delay(4 * 60 * 1000L)
                }
            }
            LaunchedEffect(account) {
                val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                val device = app.store.networkDeviceId
                if (inviteListed || account.isBlank() || refresh.isBlank() || device.isBlank()) return@LaunchedEffect
                inviteListed = true
                val listed = withContext(Dispatchers.IO) {
                    runCatching { us.spader.pjdesk.core.CloudAuth.listJoinCodes(refresh, device) }
                }
                listed.onSuccess { codes ->
                    val first = codes.firstOrNull() ?: return@onSuccess
                    if (inviteCode.isEmpty()) {
                        inviteId = first.id
                        inviteExpires = first.expiresAt
                    }
                }.onFailure { inviteError = it.message ?: "Could not list codes." }
            }
            LaunchedEffect(Unit) {
                // The service connection is what changes control. This only refreshes
                // the screen, and tells the engine when that value actually changes.
                var reported: Boolean? = null
                while (true) {
                    val enabled = PjAccessibilityService.enabled(this@MainActivity)
                    val bound = RemoteControl.service != null
                    controlEnabled = enabled
                    controlBound = bound
                    val on = enabled && bound
                    if (on != reported) {
                        reported = on
                        engine.setControl(on)
                    }
                    us.spader.pjdesk.service.ControlNotice.sync(this@MainActivity)
                    kotlinx.coroutines.delay(1_500)
                }
            }
            LaunchedEffect(state.hosting) {
                if (!state.hosting) {
                    controlDismissed = false
                    hostChat = false
                }
            }
            LaunchedEffect(state.chatSeq) {
                if (!state.chatAlert) return@LaunchedEffect
                val line = state.chatLines.lastOrNull() ?: return@LaunchedEffect
                if (state.hosting) {
                    hostChat = true
                    us.spader.pjdesk.service.ChatNotice.show(this@MainActivity, line.name, line.text)
                    engine.chatSeen()
                }
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
                if (key != null && refresh.isNotBlank() && device.isNotBlank() && app.store.syncsSavedComputers()) {
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
                engine.stageAccount(refresh, device)
                if (!temporarySession && state.server.isBlank() && app.store.accountServer.isNotBlank()) {
                    engine.applyAccountServer(app.store.accountServer, app.store.accountFingerprint, quiet = true)
                }
                if (account.isBlank() || refresh.isBlank() || device.isBlank()) return@LaunchedEffect
                val computers = if (temporarySession || !app.store.syncsSavedComputers()) null else CloudAuth.computersJson(app.store.saved())
                val outcome = withContext(Dispatchers.IO) {
                    val mine = CloudAuth.publicIp()
                    if (mine.isNotBlank()) app.store.myPublicIp = mine
                    runCatching { refreshAccount(refresh, device, computers) }
                }
                outcome.onSuccess { session ->
                    engine.noteAccountSession("valid")
                    cloudAccess = session.access
                    engine.presentAccess(session.access, session.refresh, session.deviceId)
                    if (!temporarySession) {
                        rememberAccountServer(session.address, session.fingerprint, session.lan, session.publicIp, session.standbyAddress, session.routes, session.standbyLan, session.standbyPublicIp)
                        if (app.store.syncsSavedComputers()) {
                            app.store.mergeCloud(session.computers)
                            saved = app.store.saved()
                            if (app.vaultKey != null) syncPasswords(true)
                        }
                    }
                }.onFailure { err ->
                    AppLog.e("account refresh failed: ${err.message}")
                    val next = us.spader.pjdesk.core.sessionAfterRefreshFailure(err.message.orEmpty())
                    if (next != null) engine.noteAccountSession(next)
                    if (!temporarySession && app.store.accountServer.isNotBlank()) {
                        engine.applyAccountServer(app.store.accountServer, app.store.accountFingerprint, quiet = true)
                    }
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
                val uploadLocal = !temporarySession && app.store.syncsSavedComputers()
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
                val digits = connectId.filter { it.isDigit() }
                val seen = liveSaved.firstOrNull { it.id == digits }?.lastSeen ?: 0L
                if (connecting && shouldRetryNotOnline(text, seen, System.currentTimeMillis(), notOnlineTries)) {
                    val retryId = digits
                    val retryName = connectComputerName(saved, digits)
                    val retryPassword = connectPassword
                    val retryAsk = connectAsk
                    val retryGen = connectGen
                    val retryAttempt = notOnlineTries + 1
                    AppLog.i("connect: $retryId is not online, retry $retryAttempt in 2s")
                    scope.launch {
                        delay(2_000)
                        if (!connecting || retryGen != connectGen) {
                            AppLog.i("connect retry $retryAttempt for $retryId skipped")
                            return@launch
                        }
                        notOnlineTries = retryAttempt
                        launchConnect(retryId, retryName, retryPassword, retryAsk, resetNotOnline = false)
                    }
                    engine.dismissError()
                    return@LaunchedEffect
                }
                val listedOnline = liveSaved.firstOrNull { it.id == digits }?.online == true
                val friendly = connectFailureText(text, connectComputerName(saved, connectId), listedOnline)
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
            LaunchedEffect(state.hosting, state.hostWho, state.acceptedAutomatically) {
                HostService.controlled(this@MainActivity, state.hosting, state.hostWho, state.acceptedAutomatically)
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
                if (!temporarySession && refresh.isNotBlank() && device.isNotBlank() && app.store.syncsSavedComputers()) {
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
                        pinEpoch++
                        app.bioPrompt = false
                        app.holdVaultKey(key)
                        app.store.pinFails = 0
                        app.store.pinLockedUntil = 0L
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
            fun notePinMiss() {
                val fails = app.store.pinFails + 1
                app.store.pinFails = fails
                val wait = AppLockPolicy.lockoutSeconds(fails)
                if (wait > 0) app.store.pinLockedUntil = System.currentTimeMillis() + wait * 1000L
                pinRejects++
                lockError = if (wait >= 60) {
                    val minutes = (wait + 59) / 60
                    "Too many tries. Try again in $minutes minute${if (minutes == 1) "" else "s"}."
                } else if (wait > 0) {
                    "Too many tries. Try again in $wait seconds."
                } else {
                    "Wrong PIN. ${AppLockPolicy.MAX_FAILS - fails} left before a wait."
                }
                AppLog.i("app pin rejected fails=$fails")
            }
            fun unlockWithPin(key: ByteArray?) {
                pinEpoch++
                lockBusy = false
                app.store.pinFails = 0
                app.store.pinLockedUntil = 0L
                if (key != null) app.holdVaultKey(key)
                lockError = ""
                forgotPin = false
                app.markUnlocked()
                syncPasswords(!temporarySession)
            }
            var pinEditGen by remember { mutableIntStateOf(0) }
            var pinMissGen by remember { mutableIntStateOf(-1) }
            fun notePinEdit() {
                pinEditGen++
                if (app.store.pinLockedUntil > System.currentTimeMillis()) return
                lockError = ""
            }
            fun notePinMissFor(gen: Int) {
                if (gen == pinMissGen) return
                pinMissGen = gen
                notePinMiss()
            }
            fun submitPin(pin: String) {
                if (app.store.pinLockedUntil > System.currentTimeMillis()) return
                val epoch = pinEpoch
                val gen = pinEditGen
                if (app.store.hasAccountPin()) {
                    lockBusy = true
                    lockError = ""
                    scope.launch {
                        val ok = withContext(Dispatchers.IO) {
                            AccountPin.matches(pin, app.store.accountPinSalt, app.store.accountPinHash, app.store.accountPinIters)
                        }
                        if (epoch != pinEpoch) {
                            lockBusy = false
                            return@launch
                        }
                        if (!ok) {
                            lockBusy = false
                            notePinMissFor(gen)
                            return@launch
                        }
                        val salt = app.store.pinSaltBytes()
                        val wrapped = app.store.pinWrap
                        val key = if (salt != null && wrapped.isNotBlank()) {
                            withContext(Dispatchers.IO) { PinWrap.unwrap(wrapped, pin, salt) }
                        } else {
                            null
                        }
                        if (epoch != pinEpoch) {
                            lockBusy = false
                            return@launch
                        }
                        unlockWithPin(key)
                    }
                    return
                }
                val salt = app.store.pinSaltBytes()
                val wrapped = app.store.pinWrap
                if (salt == null || wrapped.isBlank()) return
                lockBusy = true
                lockError = ""
                scope.launch {
                    val key = withContext(Dispatchers.IO) { PinWrap.unwrap(wrapped, pin, salt) }
                    if (epoch != pinEpoch) {
                        lockBusy = false
                        return@launch
                    }
                    if (key == null) {
                        lockBusy = false
                        notePinMissFor(gen)
                        return@launch
                    }
                    unlockWithPin(key)
                }
            }
            fun peekPin(pin: String) {
                if (app.store.pinLockedUntil > System.currentTimeMillis()) return
                val epoch = pinEpoch
                val gen = pinEditGen
                val account = app.store.hasAccountPin()
                scope.launch {
                    val result = withContext(Dispatchers.IO) {
                        if (account) {
                            val match = AccountPin.matches(
                                pin,
                                app.store.accountPinSalt,
                                app.store.accountPinHash,
                                app.store.accountPinIters,
                            )
                            if (!match) return@withContext null to false
                            val salt = app.store.pinSaltBytes()
                            val wrapped = app.store.pinWrap
                            val key = if (salt != null && wrapped.isNotBlank()) PinWrap.unwrap(wrapped, pin, salt) else null
                            key to true
                        } else {
                            val salt = app.store.pinSaltBytes()
                            val wrapped = app.store.pinWrap
                            if (salt == null || wrapped.isBlank()) return@withContext null to false
                            val key = PinWrap.unwrap(wrapped, pin, salt)
                            key to (key != null)
                        }
                    }
                    if (epoch != pinEpoch) return@launch
                    val (key, ok) = result
                    if (!ok) {
                        if (gen == pinEditGen && AppLockPolicy.pinMissCounts(pin.length, false)) notePinMissFor(gen)
                        return@launch
                    }
                    unlockWithPin(key)
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
                                val signed = signInAccount(email, password, device)
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
                            rememberAccountServer(signed.address, signed.fingerprint, signed.lan, signed.publicIp, signed.standbyAddress, signed.routes, signed.standbyLan, signed.standbyPublicIp)
                            cloudAccess = signed.access
                            engine.presentAccess(signed.access, signed.refresh, signed.deviceId)
                            commitAutofill()
                        }
                        val salt = PinWrap.newSalt()
                        app.store.savePinSalt(salt)
                        app.store.pinWrap = PinWrap.wrap(key, pin, salt)
                        app.store.localPinLen = pin.length
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
                            CloudAuth.confirmPassword(email, password)
                            CloudAuth.vaultKey(email, password)
                        }
                    }
                    lockBusy = false
                    result.onSuccess { key ->
                        app.holdVaultKey(key)
                        val lock = app.store.appLockSealed
                        if (lock.isNotBlank()) {
                            val verifier = runCatching { AccountPin.open(key, lock) }.getOrNull()
                            if (verifier != null) {
                                app.store.saveAccountPin(email, verifier, lock, app.store.appLockUpdated, app.store.appLockPending)
                            } else if (!app.store.hasAccountPin()) {
                                lockError = "That PIN was sealed with a different account password."
                                return@onSuccess
                            }
                        }
                        app.store.pinFails = 0
                        app.store.pinLockedUntil = 0L
                        vaultRejected = false
                        lockError = ""
                        changingPin = false
                        forgotPin = false
                        pinPending = false
                        app.markUnlocked()
                    }.onFailure {
                        lockError = it.message ?: "Could not sign in."
                    }
                }
            }
            LaunchedEffect(account, temporarySession, uiUnlocked) {
                val persistent = account.isNotEmpty() && !temporarySession
                val bioMatches = !app.store.hasAccountPin() || app.store.bioForUpdated == app.store.accountPinUpdated
                if (!persistent || uiUnlocked || !app.store.biometricsOn || app.store.bioWrap.isBlank() || !bioMatches) return@LaunchedEffect
                if (app.store.pinLockedUntil > System.currentTimeMillis()) return@LaunchedEffect
                tryBio()
            }
            fun pullAppLock() {
                if (!CloudGate.serverUp) return
                if (app.store.joinedWithoutPassword || visitWithoutPassword) return
                val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                val device = app.store.networkDeviceId
                if (refresh.isBlank() || device.isBlank()) return
                scope.launch {
                    val remote = withContext(Dispatchers.IO) {
                        if (app.store.appLockPending && app.store.appLockSealed.isNotBlank()) {
                            runCatching { CloudAuth.putAppLock(refresh, device, app.store.appLockSealed) }
                        }
                        runCatching { CloudAuth.fetchAppLock(refresh, device) }
                    }.getOrNull() ?: return@launch
                    val (lock, updated) = remote
                    val pending = app.store.appLockPending
                    val localSealed = app.store.appLockSealed
                    val localGen = app.store.accountPinUpdated
                    val localHash = app.store.accountPinHash
                    val localUpdated = app.store.appLockUpdated
                    val key = app.vaultKey
                    val opened = if (lock.isNotBlank() && key != null) {
                        runCatching { AccountPin.open(key, lock) }.getOrNull()
                    } else {
                        null
                    }
                    val remoteGen = opened?.updated ?: 0L
                    val remoteHash = opened?.hash.orEmpty()
                    val sameSealed = lock.isNotBlank() && lock == localSealed
                    val sameHash = localHash.isNotBlank() && remoteHash.isNotBlank() && localHash == remoteHash
                    // Same rule as pjdesk_proto::applock::adopt_lock. A PIN saved on this
                    // phone stays until the account has that same PIN or a later one.
                    if (lock.isBlank() && remoteHash.isBlank()) {
                        if (updated == 0L || pending) return@launch
                        if (updated > maxOf(localUpdated, localGen)) {
                            app.store.clearAccountPin()
                            app.store.clearPinMaterial()
                            BioVault.forget()
                            pinPending = false
                            app.markUnlocked()
                        }
                        return@launch
                    }
                    if (sameSealed || (sameHash && (pending || remoteGen <= localGen))) {
                        if (pending && sameSealed) {
                            app.store.appLockPending = false
                            if (updated > app.store.appLockUpdated) app.store.appLockUpdated = updated
                        }
                        return@launch
                    }
                    if (pending && (localGen > 0L || localHash.isNotBlank())) return@launch
                    if (opened == null) {
                        if (localGen == 0L && localHash.isBlank() && localSealed.isBlank() && lock.isNotBlank()) {
                            app.store.notePendingAppLock(lock, updated)
                            pinPending = true
                        }
                        return@launch
                    }
                    if (remoteGen <= localGen) return@launch
                    val changed = app.store.hasAccountPin() && app.store.accountPinHash != opened.hash
                    app.store.saveAccountPin(account.ifBlank { netEmail }, opened, lock, updated.coerceAtLeast(opened.updated))
                    pinPending = false
                    if (changed) {
                        app.store.bioWrap = ""
                        app.store.bioForUpdated = 0L
                        app.lockUi()
                    }
                }
            }
            LaunchedEffect(account, temporarySession) {
                if (account.isBlank() || temporarySession) return@LaunchedEffect
                while (true) {
                    pullAppLock()
                    delay(45_000)
                }
            }
            LaunchedEffect(bumps, uiUnlocked, account) {
                if (!uiUnlocked || account.isBlank() || temporarySession) return@LaunchedEffect
                if (!app.store.hasAccountPin() && !app.store.hasPin()) return@LaunchedEffect
                delay(app.store.autoLockSeconds * 1000L)
                if (app.uiUnlocked.value && !app.bioPrompt) app.lockUi()
            }
            fun saveAccountPin(current: String, password: String, newPin: String, idleSec: Int, enableBio: Boolean) {
                val email = netEmail.ifBlank { account }.trim().ifBlank { "local" }
                if (app.store.joinedWithoutPassword || visitWithoutPassword) {
                    lockBusy = true
                    pinFormError = ""
                    scope.launch {
                        val result = withContext(Dispatchers.IO) {
                            runCatching {
                                val have = app.store.hasAccountPin()
                                if (have && (current.isBlank() || !AccountPin.matches(current, app.store.accountPinSalt, app.store.accountPinHash, app.store.accountPinIters))) {
                                    error("That PIN is wrong.")
                                }
                                AccountPin.create(newPin, idleSec, System.currentTimeMillis() / 1000)
                            }
                        }
                        lockBusy = false
                        result.onSuccess { verifier ->
                            app.store.saveAccountPin(email, verifier, "", verifier.updated)
                            pinFormError = ""
                            app.markUnlocked()
                        }.onFailure {
                            pinFormError = it.message ?: "Could not save the PIN."
                        }
                    }
                    return
                }
                lockBusy = true
                pinFormError = ""
                scope.launch {
                    val result = withContext(Dispatchers.IO) {
                        runCatching {
                            val key = if (password.isNotBlank()) {
                                CloudAuth.confirmPassword(email, password)
                                CloudAuth.vaultKey(email, password)
                            } else {
                                app.vaultKey?.copyOf() ?: error("Enter your account password.")
                            }
                            if (app.store.hasAccountPin()) {
                                if (current.isBlank() && password.isBlank()) error("Enter the current PIN or your account password.")
                                if (password.isBlank() && !AccountPin.matches(current, app.store.accountPinSalt, app.store.accountPinHash, app.store.accountPinIters)) {
                                    error("That PIN is wrong.")
                                }
                            } else if (password.isBlank()) {
                                error("Enter your account password.")
                            }
                            val verifier = AccountPin.create(newPin, idleSec, System.currentTimeMillis() / 1000)
                            val sealed = AccountPin.seal(key, verifier)
                            val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                            val device = app.store.networkDeviceId
                            val stored = if (refresh.isBlank() || device.isBlank()) {
                                null to "PIN saved on this phone. It will sync to the account when the server answers."
                            } else {
                                runCatching { CloudAuth.putAppLock(refresh, device, sealed) }.fold(
                                    onSuccess = { it to "" },
                                    onFailure = { err ->
                                        val why = err.message.orEmpty()
                                        val saved = "PIN saved on this phone. It will sync to the account when the server answers."
                                        val note = if (why.contains("Sign in required") || why.contains("requires sign-in")) {
                                            "$why\n$saved"
                                        } else {
                                            saved
                                        }
                                        null to note
                                    },
                                )
                            }
                            Triple(key, verifier, sealed to stored)
                        }
                    }
                    lockBusy = false
                    result.onSuccess { (key, verifier, stored) ->
                        val (sealed, outcome) = stored
                        val (updated, note) = outcome
                        val whenSaved = updated ?: (System.currentTimeMillis() / 1000)
                        app.store.saveAccountPin(email, verifier, sealed, whenSaved, updated == null)
                        val salt = PinWrap.newSalt()
                        app.store.savePinSalt(salt)
                        app.store.pinWrap = PinWrap.wrap(key, newPin, salt)
                        app.holdVaultKey(key)
                        pinFormError = note
                        if (enableBio && BioVault.available(this@MainActivity)) {
                            app.bioPrompt = true
                            BioVault.sealWithAuth(
                                this@MainActivity,
                                key,
                                onSealed = { wrap ->
                                    app.bioPrompt = false
                                    app.store.bioWrap = wrap
                                    app.store.biometricsOn = true
                                    app.store.bioForUpdated = whenSaved
                                },
                                onCancel = { app.bioPrompt = false },
                                onError = {
                                    app.bioPrompt = false
                                    pinFormError = it
                                },
                            )
                        }
                    }.onFailure {
                        pinFormError = it.message ?: "Could not save the PIN."
                    }
                }
            }
            fun removeAccountPin(current: String, password: String) {
                if (app.store.joinedWithoutPassword || visitWithoutPassword) {
                    if (app.store.hasAccountPin() && !AccountPin.matches(current, app.store.accountPinSalt, app.store.accountPinHash, app.store.accountPinIters)) {
                        pinFormError = "That PIN is wrong."
                        return
                    }
                    app.store.clearAccountPin()
                    app.store.clearPinMaterial()
                    BioVault.forget()
                    pinFormError = ""
                    pinPending = false
                    app.markUnlocked()
                    return
                }
                val email = netEmail.ifBlank { account }.trim()
                lockBusy = true
                pinFormError = ""
                scope.launch {
                    val result = withContext(Dispatchers.IO) {
                        runCatching {
                            if (password.isNotBlank()) CloudAuth.confirmPassword(email, password)
                            else if (!AccountPin.matches(current, app.store.accountPinSalt, app.store.accountPinHash, app.store.accountPinIters)) {
                                error("That PIN is wrong.")
                            }
                            val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                            val device = app.store.networkDeviceId
                            if (refresh.isBlank() || device.isBlank()) error("Sign in again, then remove the PIN.")
                            CloudAuth.putAppLock(refresh, device, "")
                        }
                    }
                    lockBusy = false
                    result.onSuccess {
                        app.store.clearAccountPin()
                        app.store.clearPinMaterial()
                        BioVault.forget()
                        pinFormError = ""
                        app.markUnlocked()
                    }.onFailure {
                        pinFormError = it.message ?: "Could not remove the PIN."
                    }
                }
            }
            PjTheme(theme.value) {
                val view = LocalView.current
                val lightBars = androidx.compose.material3.MaterialTheme.colorScheme.background.luminance() > 0.5f
                SideEffect {
                    WindowCompat.getInsetsController(this@MainActivity.window, view).apply {
                        isAppearanceLightStatusBars = lightBars
                        isAppearanceLightNavigationBars = lightBars
                    }
                }
                val session = state.session
                val persistent = account.isNotEmpty() && !temporarySession
                val noPassword = app.store.joinedWithoutPassword || visitWithoutPassword
                val bioAvailable = BioVault.available(this@MainActivity)
                val pinWait = app.store.pinLockedUntil > System.currentTimeMillis()
                val gate = when {
                    !persistent && !noPassword -> null
                    noPassword && !app.store.hasAccountPin() && !app.store.hasPin() -> null
                    forgotPin && !noPassword -> "reauth"
                    changingPin -> "change"
                    (app.store.hasAccountPin() || app.store.hasPin()) && !uiUnlocked -> "unlock"
                    pinPending && !uiUnlocked && !noPassword -> "reauth"
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
                    LaunchedEffect(Unit) { pinEpoch++ }
                    UnlockScreen(
                        busy = lockBusy,
                        error = lockError,
                        biometrics = bioAvailable && app.store.biometricsOn && app.store.bioWrap.isNotBlank()
                            && (!app.store.hasAccountPin() || app.store.bioForUpdated == app.store.accountPinUpdated),
                        rejectCount = pinRejects,
                        lockedOut = pinWait,
                        knownLen = if (app.store.hasAccountPin()) app.store.accountPinLen else app.store.localPinLen,
                        onSubmit = { submitPin(it) },
                        onSilent = { peekPin(it) },
                        onEdit = { notePinEdit() },
                        onBiometric = { tryBio() },
                        computerId = state.idFormatted.ifBlank { state.id },
                        oneTimePassword = state.password,
                        permanentSet = state.hasPermanent,
                        onCopyId = { copy(state.id.ifBlank { state.idFormatted.filter { !it.isWhitespace() } }) },
                        onCopyPassword = { copy(state.password) },
                        onRefreshPassword = { engine.regenerate() },
                        onForgot = {
                            if (noPassword) {
                                app.store.clearAccountPin()
                                app.store.clearPinMaterial()
                                BioVault.forget()
                                pinPending = false
                                lockError = ""
                                app.markUnlocked()
                            } else {
                                forgotPin = true
                                lockError = ""
                            }
                        },
                    )
                } else if (gate == "reauth") {
                    ReauthScreen(
                        email = netEmail.ifBlank { account },
                        onEmail = { netEmail = it },
                        busy = lockBusy,
                        error = lockError,
                        onSubmit = { reauth(it) },
                        onCancel = if (forgotPin) {
                            { forgotPin = false; lockError = "" }
                        } else {
                            null
                        },
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
                        reconnecting = state.reconnecting,
                        reconnectSeconds = state.reconnectSeconds,
                        onReconnectNow = engine::reconnectNow,
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
                        chatReady = state.chatReady,
                        chatLines = state.chatLines,
                        chatUnread = state.chatUnread,
                        onChat = engine::chat,
                        onChatOpen = engine::chatSeen,
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
                } else if (signingIn || (account.isEmpty() && state.server.isEmpty())) {
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
                        signInPaused = signInPauseUntil > android.os.SystemClock.elapsedRealtime(),
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
                        onClose = if (account.isNotEmpty() || state.server.isNotEmpty()) {
                            { signingIn = false }
                        } else {
                            null
                        },
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
                                    runCatching { signInAccount(email, pass, device) }
                                }
                                result.onSuccess { session ->
                                    applySignedIn(session, email, null, false, true)
                                }.onFailure { err ->
                                    netBusy = false
                                    if (!noteSignInPause(err)) netError = us.spader.pjdesk.core.signInPageError(err.message ?: "Could not sign in.")
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
                                    netError = us.spader.pjdesk.core.signInPageError(it.message ?: "Could not sign in.")
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
                                            else -> signInAccount(email, pass, device)
                                        }
                                    }
                                }
                                result.onSuccess { session ->
                                    val accountPassword = when {
                                        useKey -> if (bound) pass else ""
                                        useCode -> ""
                                        else -> pass
                                    }
                                    applySignedIn(
                                        session,
                                        email,
                                        if (useKey) blob else null,
                                        signOutWhenDone,
                                        false,
                                        accountPassword = accountPassword,
                                        withoutPassword = useCode || useKey,
                                    )
                                }.onFailure { err ->
                                    netBusy = false
                                    val cloud = err as? CloudAuthException
                                    netOffer = cloud != null && cloud.code == "no_server" && CloudAuth.offerCurrentServer(
                                        if (cloud.role.isBlank()) "owner" else cloud.role,
                                        state.server,
                                        state.fingerprint,
                                    )
                                    netLogin = if (netOffer) cloud?.loginToken.orEmpty() else ""
                                    if (!noteSignInPause(err)) netError = us.spader.pjdesk.core.signInPageError(err.message ?: "Could not sign in.")
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
                        contentWindowInsets = WindowInsets.safeDrawing,
                        topBar = {
                            TopAppBar(
                                windowInsets = WindowInsets.safeDrawing.only(WindowInsetsSides.Horizontal + WindowInsetsSides.Top),
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
                            PjBottomBar(selected = tab, onSelect = { tab = it })
                        },
                    ) { padding ->
                        androidx.compose.foundation.layout.Box(Modifier.padding(padding).fillMaxSize()) {
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
                                    onSignIn = if (account.isEmpty() || state.accountSession != "valid") {
                                        { signingIn = true }
                                    } else {
                                        null
                                    },
                                    accountEmail = account,
                                    accountSession = state.accountSession,
                                    onSignOut = if (account.isEmpty()) {
                                        null
                                    } else {
                                        {
                                            signOutAndClearSaved()
                                            engine.noteAccountSession("")
                                            scope.launch { snackbar.showSnackbar("Signed out. Saved computers were removed from this computer") }
                                        }
                                    },
                                    temporary = temporarySession,
                                    recent = liveSaved.filter { recentIds.contains(it.id) }.take(3),
                                    onRecent = { beginConnect(it) },
                                    selfId = state.id,
                                    onRefreshSupport = { engine.refreshSupport() },
                                    onSupportConnect = { code, password ->
                                        when {
                                            state.incomingOnly -> scope.launch {
                                                snackbar.showSnackbar("Incoming only is on. This computer can't connect to others.")
                                            }
                                            code.length != 8 || password.isBlank() -> scope.launch {
                                                snackbar.showSnackbar("Enter the 8-digit code and the temporary password.")
                                            }
                                            else -> scope.launch {
                                                val found = withContext(Dispatchers.IO) {
                                                    runCatching { us.spader.pjdesk.core.CloudAuth.lookupSupport(code) }
                                                }
                                                found.onSuccess { id -> launchConnect(id, "", password) }
                                                    .onFailure { snackbar.showSnackbar(it.message ?: "That code is not valid.") }
                                            }
                                        }
                                    },
                                    inviteCode = inviteCode,
                                    inviteExpiresAt = inviteExpires,
                                    inviteId = inviteId,
                                    inviteBusy = inviteBusy,
                                    inviteError = inviteError,
                                    onCreateInvite = {
                                        val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                                        val device = app.store.networkDeviceId
                                        if (refresh.isBlank() || device.isBlank()) {
                                            inviteError = "Sign in again."
                                        } else {
                                            inviteBusy = true
                                            inviteError = ""
                                            scope.launch {
                                                val result = withContext(Dispatchers.IO) {
                                                    runCatching { us.spader.pjdesk.core.CloudAuth.createJoinCode(refresh, device) }
                                                }
                                                inviteBusy = false
                                                result.onSuccess {
                                                    inviteCode = it.code
                                                    inviteId = it.id
                                                    inviteExpires = it.expiresAt
                                                }.onFailure { inviteError = it.message ?: "Could not create a code." }
                                            }
                                        }
                                    },
                                    onIncomingOnly = { on, password ->
                                        engine.command(
                                            org.json.JSONObject()
                                                .put("cmd", "incomingOnly")
                                                .put("on", on)
                                                .put("password", password)
                                                .toString(),
                                        )
                                    },
                                    onRevokeInvite = {
                                        val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                                        val device = app.store.networkDeviceId
                                        val id = inviteId
                                        if (refresh.isBlank() || device.isBlank() || id.isBlank()) {
                                            inviteError = "Sign in again."
                                        } else {
                                            inviteBusy = true
                                            inviteError = ""
                                            scope.launch {
                                                val result = withContext(Dispatchers.IO) {
                                                    runCatching { us.spader.pjdesk.core.CloudAuth.revokeJoinCode(refresh, device, id) }
                                                }
                                                inviteBusy = false
                                                result.onSuccess {
                                                    inviteCode = ""
                                                    inviteId = ""
                                                    inviteExpires = 0
                                                }.onFailure { inviteError = it.message ?: "Could not revoke the code." }
                                            }
                                        }
                                    },
                                )
                                1 -> SavedScreen(
                                    signedIn = account.isNotEmpty(),
                                    latestRelease = state.latestRelease,
                                    selfVersion = state.selfVersion,
                                    computers = liveSaved,
                                    refreshing = presenceBusy,
                                    onRefresh = { pullPresence() },
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
                                    view = savedView,
                                    onView = { chosen -> scope.launch { listPrefs.setView(chosen) } },
                                    discovered = discovered,
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
                                    onRemoveAll = {
                                        app.store.clearLocalSaved()
                                        app.store.savedSyncChoice = "no"
                                        saved = app.store.saved()
                                        recentIds = app.store.recentIds()
                                        scope.launch { snackbar.showSnackbar("Removed saved computers from this computer") }
                                    },
                                    onSignOutRemove = {
                                        signOutAndClearSaved()
                                        scope.launch { snackbar.showSnackbar("Signed out. Saved computers were removed from this computer") }
                                    },
                                    showLoadSaved = account.isNotBlank() && !app.store.syncsSavedComputers() && !state.incomingOnly,
                                    onLoadSaved = { loadMySavedComputers() },
                                    incomingOnly = state.incomingOnly,
                                    onRename = { computer, nickname ->
                                        app.store.rename(computer.id, nickname)
                                        saved = app.store.saved()
                                        val refresh = app.store.networkRefresh()
                                        val device = app.store.networkDeviceId
                                        if (refresh.isNotBlank() && device.isNotBlank() && app.store.syncsSavedComputers()) {
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
                                    savedAccessKey = app.store.accessKey,
                                    onAccessKey = { engine.setAccessKey(it) },
                                    onResetFingerprint = engine::resetFingerprint,
                                    onSavePermanent = { password, confirm -> savePermanent(password, confirm) },
                                    onClearPermanent = { removePermanent() },
                                    controlEnabled = controlEnabled,
                                    controlBound = controlBound,
                                    onOpenControl = { PjAccessibilityService.openSettings(this@MainActivity) },
                                    onUnattended = { engine.setApproval(if (it) "unattended" else "prompt") },
                                    onUnattendedAccess = { requestUnattended(it) },
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
                                    localPin = app.store.joinedWithoutPassword || visitWithoutPassword,
                                    hasPin = app.store.hasPin() || app.store.hasAccountPin(),
                                    biometricsOn = app.store.biometricsOn,
                                    biometricsAvailable = BioVault.available(this@MainActivity),
                                    autoLockSeconds = app.store.autoLockSeconds,
                                    onChangePin = { changingPin = true; lockError = "" },
                                    onSaveAccountPin = { current, password, pin, idle, bio ->
                                        saveAccountPin(current, password, pin, idle, bio)
                                    },
                                    onRemoveAccountPin = { current, password -> removeAccountPin(current, password) },
                                    pinBusy = lockBusy,
                                    pinError = pinFormError,
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
                                    onAutoLock = { seconds ->
                                        app.store.autoLockSeconds = seconds
                                        val key = app.vaultKey
                                        if (app.store.hasAccountPin() && key != null) {
                                            val email = account.ifBlank { netEmail }
                                            val refresh = if (temporarySession) memoryRefresh else app.store.networkRefresh()
                                            val device = app.store.networkDeviceId
                                            scope.launch(Dispatchers.IO) {
                                                runCatching {
                                                    val verifier = AccountPin.Verifier(
                                                        app.store.accountPinSalt,
                                                        app.store.accountPinHash,
                                                        app.store.accountPinIters,
                                                        seconds,
                                                        System.currentTimeMillis() / 1000,
                                                        app.store.accountPinLen,
                                                    )
                                                    val sealed = AccountPin.seal(key, verifier)
                                                    val updated = runCatching { CloudAuth.putAppLock(refresh, device, sealed) }.getOrNull()
                                                    val whenSaved = updated ?: (System.currentTimeMillis() / 1000)
                                                    app.store.saveAccountPin(email, verifier, sealed, whenSaved, updated == null)
                                                }
                                            }
                                        }
                                    },
                                    remote = remote,
                                    onRemote = {
                                        remote = it
                                        app.store.saveRemoteGlobal(it)
                                    },
                                    engineDetail = readiness.detail,
                                    accountEmail = account,
                                    accountSession = state.accountSession,
                                    temporary = temporarySession,
                                    onSignIn = if (account.isEmpty() || state.accountSession != "valid") {
                                        { signingIn = true }
                                    } else {
                                        null
                                    },
                                    savedCount = saved.size,
                                    onRemoveAllSaved = {
                                        app.store.clearLocalSaved()
                                        app.store.savedSyncChoice = "no"
                                        saved = app.store.saved()
                                        recentIds = app.store.recentIds()
                                        scope.launch { snackbar.showSnackbar("Removed saved computers from this computer") }
                                    },
                                    onSignOutAndRemove = {
                                        signOutAndClearSaved()
                                        scope.launch { snackbar.showSnackbar("Signed out. Saved computers were removed from this computer") }
                                    },
                                    showLoadSaved = account.isNotBlank() && !app.store.syncsSavedComputers() && !state.incomingOnly,
                                    onLoadSaved = { loadMySavedComputers() },
                                    onSignOut = if (account.isEmpty()) {
                                        null
                                    } else {
                                        {
                                            signOutAndClearSaved()
                                            engine.noteAccountSession("")
                                            scope.launch { snackbar.showSnackbar("Signed out. Saved computers were removed from this computer") }
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
                            if (state.hosting) {
                                if (!hostChat) {
                                    Column(
                                        Modifier.align(Alignment.BottomEnd).padding(12.dp).widthIn(max = 240.dp),
                                        horizontalAlignment = Alignment.End,
                                    ) {
                                        if (!state.chatReady) {
                                            Text(
                                                us.spader.pjdesk.ui.CHAT_NEEDS_TIP,
                                                color = androidx.compose.material3.MaterialTheme.colorScheme.onSurfaceVariant,
                                                style = androidx.compose.material3.MaterialTheme.typography.bodySmall,
                                                textAlign = TextAlign.End,
                                            )
                                        }
                                        TextButton(
                                            onClick = { hostChat = true },
                                            enabled = state.chatReady,
                                        ) { Text("Chat") }
                                    }
                                } else {
                                    androidx.compose.foundation.layout.Box(
                                        Modifier
                                            .fillMaxSize()
                                            .background(Color.Black.copy(alpha = 0.45f))
                                            .clickable { hostChat = false },
                                    )
                                    androidx.compose.foundation.layout.Box(Modifier.align(Alignment.BottomCenter)) {
                                        us.spader.pjdesk.ui.ChatSheet(
                                            lines = state.chatLines,
                                            ready = state.chatReady,
                                            onSend = engine::chat,
                                            onClose = { hostChat = false },
                                        )
                                    }
                                }
                            }
                        }
                    }
                }
                if (confirmUnattended) {
                    AlertDialog(
                        onDismissRequest = { confirmUnattended = false },
                        title = { Text("Turn on unattended access") },
                        text = {
                            Column {
                                Text("Enter your PJDesk PIN. Fingerprint or face works too, when this phone has it.")
                                OutlinedTextField(
                                    value = unattendedPin,
                                    onValueChange = { unattendedPin = it.filter { ch -> ch.isDigit() }.take(8) },
                                    label = { Text("PIN") },
                                    singleLine = true,
                                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                                )
                            }
                        },
                        confirmButton = {
                            TextButton(
                                onClick = {
                                    if (pinMatches(unattendedPin)) {
                                        confirmUnattended = false
                                        engine.setUnattendedAccess(true)
                                    } else {
                                        scope.launch { snackbar.showSnackbar("Wrong PIN.") }
                                    }
                                },
                                enabled = AppLockPolicy.pinAcceptable(unattendedPin),
                            ) { Text("Turn on") }
                        },
                        dismissButton = {
                            TextButton(onClick = { confirmUnattended = false }) { Text("Cancel") }
                        },
                    )
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
                        title = { Text(if (failure == "Your sign-in expired") "Your sign-in expired" else "Couldn't connect") },
                        text = { Text(failure) },
                        confirmButton = {
                            val retry = offlineRetry
                            when {
                                failure == "Your sign-in expired" -> TextButton(onClick = {
                                    connectError = null
                                    offlineRetry = null
                                    signingIn = true
                                }) { Text("Sign in") }
                                retry != null -> TextButton(onClick = {
                                    offlineRetry = null
                                    connectError = null
                                    beginConnect(retry.id, ignoreOffline = true)
                                }) { Text("Try anyway") }
                                else -> TextButton(onClick = { connectError = null }) { Text("OK") }
                            }
                        },
                        dismissButton = if (offlineRetry != null || failure == "Your sign-in expired") {
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

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        AppLog.i("main activity reused task=$taskId")
    }

    override fun onDestroy() {
        AppLog.i("main activity destroyed finishing=$isFinishing task=$taskId")
        if (isFinishing && !isChangingConfigurations && app.engine.state.value.session != null) {
            app.engine.stopView("closed")
        }
        if (isFinishing && !isChangingConfigurations) {
            val desk = app.engine.state.value
            // Rotate first so the password on screen stops working, then drop
            // the hash that was published for that pair. The snapshot is the
            // old code; refresh replaces it.
            app.engine.refreshSupport()
            Thread {
                us.spader.pjdesk.core.CloudAuth.revokeSupport(desk.supportCode, desk.id)
            }.start()
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

    private fun signInAccount(email: String, password: String, device: String): us.spader.pjdesk.core.Session {
        val known = app.engine.state.value.server.ifBlank { app.store.accountServer }
        if (known.isNotBlank() && app.engine.state.value.server.isBlank()) {
            app.engine.applyAccountServer(known, app.store.accountFingerprint, quiet = true)
        }
        val targets = LinkedHashSet<String>()
        targets.add("")
        listOf(app.engine.state.value.server, app.store.accountServer, app.store.accountStandby)
            .filter { it.isNotBlank() }
            .forEach { targets.add(it) }
        val auth = CloudAuth.authKeyHex(email, password)
        val tried = ArrayList<String>()
        for (target in targets) {
            if (target.isNotBlank() && tried.any { sameServer(it, target) }) continue
            AppLog.i(us.spader.pjdesk.core.signInLogLine(target, "request", "op=sign-in"))
            val body = JSONObject()
                .put("op", "sign-in")
                .put("email", email.trim().lowercase())
                .put("auth_key", auth)
                .put("device_id", device)
                .put("device_name", "Android")
            if (target.isNotBlank()) body.put("address", target)
            val reply = CloudGate.request(body)
            val server = reply?.optString("server").orEmpty().ifBlank { target.ifBlank { known } }
            val message = reply?.optString("message").orEmpty()
            val ok = reply?.optBoolean("ok") == true
            AppLog.i(us.spader.pjdesk.core.signInLogLine(server, "reply", "ok=$ok message=$message"))
            if (reply == null) continue
            if (ok) return CloudAuth.sessionFromServer(email, device, reply)
            if (server.isNotBlank()) tried.add(server)
            if (!us.spader.pjdesk.core.serverHasNoAccountCopy(message)) {
                val pause = reply.optLong("pause")
                val mapped = us.spader.pjdesk.core.namedServerSignInFailure(
                    server,
                    us.spader.pjdesk.core.signInPageError(message, pause),
                )
                val secs = us.spader.pjdesk.core.signInPauseSeconds(
                    if (pause > 0L) 429 else 0,
                    message.ifBlank { mapped },
                    pause.takeIf { it > 0L },
                )
                throw CloudAuthException(mapped, status = if (secs != null) 429 else 401, retryAfter = secs)
            }
        }
        val who = tried.joinToString(", ").ifBlank { known.ifBlank { "the PJDesk server" } }
        AppLog.i(us.spader.pjdesk.core.signInLogLine("pjdesk.com", "cloud", "after=$who"))
        return try {
            CloudAuth.signIn(email, password, device, "Android", forceCloud = true)
        } catch (err: CloudAuthException) {
            throw CloudAuthException(
                "The server at $who answered and does not have the account yet. ${err.message ?: "pjdesk.com did not answer."}",
            )
        }
    }

    private fun sameServer(left: String, right: String): Boolean {
        fun host(text: String) = text.trim().substringBefore(':').lowercase()
        val a = host(left)
        val b = host(right)
        return a.isNotEmpty() && a == b
    }

    private fun refreshAccount(
        refresh: String,
        device: String,
        computers: org.json.JSONArray?,
    ): us.spader.pjdesk.core.Session {
        val known = app.engine.state.value.server.ifBlank { app.store.accountServer }
        if (CloudGate.serverUp || known.isNotBlank()) {
            val body = JSONObject().put("op", "refresh").put("refresh", refresh).put("device_id", device)
            if (computers != null) body.put("computers", computers)
            val reply = CloudGate.request(body)
            if (reply?.optBoolean("ok") == true) return CloudAuth.sessionFromServer("", device, reply)
            val message = reply?.optString("message").orEmpty()
            if (message.contains("Sign in required") || message.contains("Wrong email")) {
                throw CloudAuthException(message.ifBlank { "Sign in required." })
            }
            if (reply == null) CloudGate.noteServer(false)
            if (known.isNotBlank() && !CloudGate.allowCloud()) {
                throw CloudAuthException("Waiting before asking pjdesk.com again.")
            }
        }
        val androidId = Settings.Secure.getString(contentResolver, Settings.Secure.ANDROID_ID).orEmpty()
        return CloudAuth.refresh(
            refresh,
            device,
            computers,
            pjdeskId = app.engine.state.value.id,
            hardwareId = CloudAuth.hardwareKey(androidId),
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
        HostService.preferAutoClick = false
        HostService.consentInProgress = true
        if (acceptAfter) app.engine.showStatus("Waiting for screen sharing")
        AppLog.i(if (acceptAfter) "asking to share the entire screen for an incoming connection" else "asking to share the entire screen during setup")
        projection.launch(us.spader.pjdesk.service.EntireScreen.captureIntent(this))
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
