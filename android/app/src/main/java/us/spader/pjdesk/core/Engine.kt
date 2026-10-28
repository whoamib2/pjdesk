package us.spader.pjdesk.core

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.UserManager
import android.provider.Settings
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeoutOrNull
import java.util.concurrent.atomic.AtomicBoolean
import org.json.JSONArray
import org.json.JSONObject
import us.spader.pjdesk.AppLog
import us.spader.pjdesk.a11y.PjAccessibilityService
import us.spader.pjdesk.data.Store
import us.spader.pjdesk.input.GestureMap
import us.spader.pjdesk.service.RemoteControl
import java.util.concurrent.atomic.AtomicLong

enum class EnginePhase { Starting, Ready, Failed }

data class EngineReadiness(val phase: EnginePhase, val detail: String)

class Engine(context: Context) : EngineListener {
    private val app = context.applicationContext
    private val store = Store(app).also { saved ->
        if (saved.migrateUnattendedAccess()) {
            AppLog.i("migrated When someone connects to unattended access")
        }
    }
    private val main = Handler(Looper.getMainLooper())
    private val controlGate = ControlGate()
    private var controlDropPosted = false
    private val controlDrop = Runnable {
        controlDropPosted = false
        val decision = controlGate.offer(false, android.os.SystemClock.elapsedRealtime())
        if (decision == false) pushControl(false)
    }
    private val handle = AtomicLong(0)
    private val startOnce = AtomicBoolean(false)
    private val pending = ArrayDeque<String>()
    private val _ready = MutableStateFlow(EngineReadiness(EnginePhase.Starting, "Starting PJDesk…"))
    val readiness: StateFlow<EngineReadiness> = _ready.asStateFlow()
    /** One retry when a registration event disagrees with the Keystore, so a stale event cannot loop. */
    private var permanentRepush = false

    private val _state = MutableStateFlow(
        DeskState(
            approval = store.approval,
            hasPermanent = store.permanent.isNotEmpty(),
            unattendedAccess = store.unattendedAccess,
        ),
    )
    val state: StateFlow<DeskState> = _state.asStateFlow()

    var onVideo: ((ByteArray, Int, Int, Boolean) -> Unit)? = null
    var onQuality: ((Int, Int, Int) -> Unit)? = null
    var onClipboard: ((String) -> Unit)? = null
    var onFiles: ((List<String>) -> Unit)? = null
    var onKeyframe: (() -> Unit)? = null

    fun start() {
        if (!startOnce.compareAndSet(false, true)) return
        if (!Native.loaded) {
            fail(Native.loadError.ifBlank { ENGINE_LIBRARY_MISSING })
            return
        }
        val androidId = Settings.Secure.getString(app.contentResolver, Settings.Secure.ANDROID_ID) ?: ""
        val host = deviceName(app)
        Thread({
            val created = try {
                Native.nativeCreate(app.filesDir.absolutePath, androidId, host, this)
            } catch (error: Throwable) {
                main.post { fail(error.message?.ifBlank { null } ?: "The PJDesk engine did not start.") }
                return@Thread
            }
            main.post {
                if (created == 0L) {
                    fail("The PJDesk engine did not start.")
                    return@post
                }
                handle.set(created)
                command(
                    JSONObject()
                        .put("cmd", "hostFacts")
                        .put("hostname", host)
                        .put("os_family", "android")
                        .put("os_version", Build.VERSION.RELEASE ?: "")
                        .put("console_user", profileName(app))
                        .put("display_name", store.computerName)
                        .toString(),
                )
                pushPermanent()
                pushAccessKey()
                command("""{"cmd":"setApproval","mode":"${store.approval}"}""")
                syncUnattendedAccess()
                pushTrusted()
                setCapturing(us.spader.pjdesk.service.HostService.capturing)
                setControl(us.spader.pjdesk.a11y.PjAccessibilityService.ready(app))
                flushCommands()
                _ready.value = EngineReadiness(EnginePhase.Ready, "Ready")
                AppLog.i("engine ready")
            }
        }, "pjdesk-start").start()
    }

    /**
     * Waits until the engine can take a connect, or returns the reason it cannot.
     * A missing library fails immediately. A start that is still running is waited out.
     */
    suspend fun awaitReady(timeoutMs: Long = 15_000): String? {
        val current = _ready.value
        if (current.phase == EnginePhase.Failed) return current.detail
        if (current.phase == EnginePhase.Ready && handle.get() != 0L) return null
        val done = withTimeoutOrNull(timeoutMs) {
            _ready.first { it.phase != EnginePhase.Starting }
        }
        if (done == null) {
            return _ready.value.detail.ifBlank { "PJDesk did not finish starting." }.let { detail ->
                if (_ready.value.phase == EnginePhase.Starting) "PJDesk did not finish starting." else detail
            }
        }
        return if (done.phase == EnginePhase.Ready && handle.get() != 0L) {
            null
        } else {
            done.detail.ifBlank { "The PJDesk engine did not start." }
        }
    }

    private fun fail(message: String) {
        val text = message.ifBlank { "The PJDesk engine did not start." }
        AppLog.e(text)
        _ready.value = EngineReadiness(EnginePhase.Failed, text)
        update { it.copy(status = text, error = text) }
    }

    /** A controller is still connected. False once that session has ended. */
    fun hostAttached(): Boolean {
        val h = handle.get()
        if (h == 0L || !Native.loaded) return false
        return runCatching { Native.nativeHostAttached(h) }.getOrDefault(false)
    }

    /** The accessibility service can turn PC input into taps and typing. */
    fun setControl(on: Boolean) {
        val now = android.os.SystemClock.elapsedRealtime()
        val decision = controlGate.offer(on, now)
        if (on) {
            controlDropPosted = false
            main.removeCallbacks(controlDrop)
        }
        if (decision != null) {
            pushControl(decision)
            return
        }
        if (!on && controlGate.holdingDrop() && !controlDropPosted) {
            controlDropPosted = true
            main.postDelayed(controlDrop, controlGate.remaining(now).coerceAtLeast(1L))
        }
    }

    private fun pushControl(on: Boolean) {
        command(JSONObject().put("cmd", "setControl").put("on", on).toString())
    }

    /** MediaProjection is producing frames, or it stopped. */
    fun setCapturing(on: Boolean) {
        command(JSONObject().put("cmd", "capturing").put("on", on).toString())
    }

    /**
     * A capture was a single app and was stopped. The PC is told to keep
     * waiting, and this phone is not left on "Accepted automatically".
     */
    fun screenWait() {
        command("""{"cmd":"screenWait"}""")
        update {
            it.copy(
                status = "Waiting for the phone to allow screen sharing",
                acceptedAutomatically = false,
            )
        }
    }

    /** The person denied screen sharing, or Android refused the consent. */
    fun captureDenied(reason: String) {
        val text = reason.ifBlank { "Screen capture permission denied" }
        command(JSONObject().put("cmd", "captureDenied").put("reason", text).toString())
    }

    fun pushTrusted() {
        val accounts = JSONArray()
        store.trustedAccounts().forEach { accounts.put(it) }
        val computers = JSONArray()
        store.trustedComputers().forEach { computers.put(it) }
        command(JSONObject().put("cmd", "setTrusted").put("accounts", accounts).put("computers", computers).toString())
    }

    fun allowAccount(email: String) {
        store.allowAccount(email)
        pushTrusted()
    }

    fun allowComputer(name: String) {
        store.allowComputer(name)
        pushTrusted()
    }

    fun forgetTrusted(kind: String, value: String) {
        store.forgetTrusted(kind, value)
        pushTrusted()
    }

    fun presentAccess(token: String, refresh: String = "", device: String = "") {
        val body = JSONObject().put("cmd", "accessToken").put("token", token)
        if (refresh.isNotBlank()) body.put("refresh", refresh)
        if (device.isNotBlank()) body.put("device", device)
        command(body.toString())
    }

    /** Gives the engine the keystore refresh before a cloud refresh finishes. Does not clear a saved access token. */
    fun stageAccount(refresh: String, device: String) {
        if (refresh.isBlank() && device.isBlank()) return
        val body = JSONObject().put("cmd", "accessToken").put("token", "")
        if (refresh.isNotBlank()) body.put("refresh", refresh)
        if (device.isNotBlank()) body.put("device", device)
        command(body.toString())
    }

    fun noteAccountSession(state: String) {
        val cleaned = when (state) {
            "valid", "expired" -> state
            else -> ""
        }
        update { it.copy(accountSession = cleaned) }
    }

    fun command(json: String) {
        val h = handle.get()
        if (h == 0L) {
            synchronized(pending) {
                if (pending.size >= 40) pending.removeFirst()
                pending.addLast(json)
            }
            return
        }
        Native.nativeCommand(h, json)
    }

    private fun flushCommands() {
        val h = handle.get()
        if (h == 0L) return
        val queued = synchronized(pending) {
            val copy = pending.toList()
            pending.clear()
            copy
        }
        queued.forEach { Native.nativeCommand(h, it) }
    }

    /** Account sync sets the address and pinned key without replacing "Connecting…". */
    fun applyAccountServer(
        address: String,
        fingerprint: String = "",
        quiet: Boolean = true,
        standby: String = "",
        routes: String = "",
        standbyLan: String = "",
        standbyPublicIp: String = "",
    ) {
        if (address.isBlank()) return
        val body = JSONObject().put("cmd", "server").put("address", address).put("quiet", quiet)
        if (fingerprint.isNotBlank()) body.put("fingerprint", fingerprint)
        if (standby.isNotBlank()) body.put("standby", standby)
        if (routes.isNotBlank()) body.put("routes", routes)
        if (standbyLan.isNotBlank()) body.put("standbyLan", standbyLan)
        if (standbyPublicIp.isNotBlank()) body.put("standbyPublicIp", standbyPublicIp)
        if (quiet) {
            body.put("lan", lanForEngine())
            body.put("publicIp", store.accountPublicIp)
            body.put("myPublicIp", store.myPublicIp)
        } else {
            body.put("lan", JSONArray())
            body.put("publicIp", "")
            body.put("myPublicIp", "")
        }
        command(body.toString())
    }

    fun submitVideo(data: ByteArray, width: Int, height: Int, keyframe: Boolean, encodeUs: Int) {
        val h = handle.get()
        if (h != 0L) Native.nativeSubmitVideo(h, data, width, height, keyframe, encodeUs)
    }

    fun submitPassword(password: String) = command(
        JSONObject().put("cmd", "submitPassword").put("password", password).toString(),
    )

    fun cancelPassword() = command("""{"cmd":"cancelPassword"}""")

    fun checkForUpdate() {
        update { it.copy(status = "Checking for updates…") }
        command("""{"cmd":"checkUpdate"}""")
    }

    /** Ask the person at the other computer instead of sending a password. */
    fun askPerson() {
        update { it.copy(askPassword = false, askNote = "", status = "Asking the person at that computer") }
        command("""{"cmd":"askPerson"}""")
    }

    /** Blank clears the friendly name so other computers see the device name. */
    fun setDisplayName(name: String) {
        val cleaned = name.trim().filter { !it.isISOControl() }.take(64)
        store.computerName = cleaned
        command(JSONObject().put("cmd", "hostFacts").put("display_name", cleaned).toString())
    }

    fun deviceLabel(): String = deviceName(app)

    fun connect(id: String, password: String, askPerson: Boolean = false, name: String = "") {
        val digits = id.filter { it.isDigit() }
        val how = when {
            askPerson -> "ask-person"
            password.isEmpty() -> "none"
            else -> "present"
        }
        AppLog.i("engine connect id=$digits password=$how handle=${handle.get()}")
        if (handle.get() == 0L) {
            val why = _ready.value.detail.ifBlank { "The PJDesk engine did not start." }
            showError(if (_ready.value.phase == EnginePhase.Failed) why else "PJDesk did not finish starting.")
            return
        }
        val email = store.networkEmail.trim()
        if (email.isNotEmpty()) {
            command(JSONObject().put("cmd", "account").put("email", email).toString())
        }
        command(
            JSONObject()
                .put("cmd", "connect")
                .put("id", digits)
                .put("password", if (askPerson) "" else password)
                .put("ask", askPerson)
                .put("name", name)
                .toString(),
        )
    }

    /**
     * Ends a session this phone is controlling. The host registration keeps
     * running, so an incoming view request can still arrive.
     */
    fun stopView(reason: String = "stopped") {
        AppLog.i("ending the controller session: $reason")
        us.spader.pjdesk.service.SessionNotice.hide(app)
        update {
            val active = it.session != null ||
                it.status.startsWith("Connecting") ||
                it.status.startsWith("Waiting") ||
                it.status.startsWith("Asking")
            it.copy(
                session = null,
                reconnecting = false,
                reconnectSeconds = 0,
                askPassword = false,
                askNote = "",
                status = if (active) "Ready" else it.status,
            )
        }
        command(JSONObject().put("cmd", "stopView").put("reason", reason).toString())
    }

    fun accept() {
        AppLog.i("accepting incoming connection")
        command("""{"cmd":"accept"}""")
    }

    fun decline(reason: String = "the person at this phone declined") {
        AppLog.i("declining incoming connection: $reason")
        command(JSONObject().put("cmd", "decline").put("reason", reason).toString())
    }

    fun stop(reason: String = "stopped") {
        AppLog.i("stopping the session: $reason")
        command(JSONObject().put("cmd", "stop").put("reason", reason).toString())
    }

    fun showStatus(text: String) {
        AppLog.i(text)
        update { it.copy(status = text) }
    }

    fun showError(text: String) {
        AppLog.e(text)
        update { it.copy(error = text, status = text) }
    }

    fun gesture(g: GestureMap.Gesture) {
        val x = JSONArray()
        val y = JSONArray()
        g.x.forEach { x.put(it) }
        g.y.forEach { y.put(it) }
        command(
            JSONObject()
                .put("cmd", "gesture")
                .put("kind", g.kind)
                .put("duration", g.durationMs)
                .put("x", x)
                .put("y", y)
                .toString(),
        )
    }

    fun mouse(kind: String, x: Int, y: Int, dy: Int = 0, button: String = "left") {
        command(
            JSONObject()
                .put("cmd", "mouse")
                .put("kind", kind)
                .put("button", button)
                .put("x", x)
                .put("y", y)
                .put("dy", dy)
                .toString(),
        )
    }

    fun chat(text: String) {
        if (text.isBlank()) return
        command(JSONObject().put("cmd", "chat").put("text", text.take(2000)).toString())
    }

    fun chatSeen() = command("""{"cmd":"chatSeen"}""")

    fun paste(text: String) {
        if (text.isBlank()) return
        command(JSONObject().put("cmd", "paste").put("text", text.take(48_000)).toString())
    }

    private fun lanForEngine(): JSONArray {
        val lan = JSONArray()
        val seen = HashSet<String>()
        fun add(raw: String) {
            val text = raw.trim()
            if (text.isEmpty() || !seen.add(text)) return
            lan.put(text)
        }
        store.accountLan.split(',').forEach { add(it) }
        add(store.lanPath(NetworkPath.key(app)))
        return lan
    }

    fun text(chars: String) = command(JSONObject().put("cmd", "text").put("chars", chars).toString())
    fun special(which: Int) = command(JSONObject().put("cmd", "special").put("which", which).toString())
    fun key(scan: Int, down: Boolean) = command(JSONObject().put("cmd", "key").put("scan", scan).put("down", down).toString())
    fun releaseKeys() = command("""{"cmd":"release"}""")
    fun quality(name: String) = command(JSONObject().put("cmd", "quality").put("name", name).toString())
    fun qualityCustom(fps: Int, bitrate: Int) = command(
        JSONObject()
            .put("cmd", "quality")
            .put("name", "custom")
            .put("fps", fps.coerceIn(5, 60))
            .put("bitrate", bitrate.coerceIn(200, 20_000))
            .put("maxWidth", 1920)
            .toString(),
    )
    fun privacy(enabled: Boolean) = command(JSONObject().put("cmd", "privacy").put("enabled", enabled).toString())
    fun keyboardMode(mode: String) = command(JSONObject().put("cmd", "keyboardMode").put("mode", mode).toString())
    fun restart() = command("""{"cmd":"restart"}""")
    fun ping(tUs: Long) = command(JSONObject().put("cmd", "ping").put("t_us", tUs).toString())
    fun fileList() = command("""{"cmd":"fileList"}""")
    fun sendFile(path: String) = command(JSONObject().put("cmd", "fileSend").put("path", path).toString())
    fun display(index: Int, layout: Boolean) =
        command(JSONObject().put("cmd", "display").put("index", index).put("layout", layout).toString())

    /** Dials the open session again without waiting out the current pause. */
    fun reconnectNow() = command("""{"cmd":"reconnectNow"}""")

    fun keyframe() = command("""{"cmd":"keyframe"}""")

    /**
     * Saves [password] in the Keystore and gives it to the running host.
     * An empty password removes it. Returns false when it could not be stored.
     * The password itself is never written to the log.
     */
    fun setPermanent(password: String, quiet: Boolean = false): Boolean {
        val trimmed = password.trim()
        if (trimmed.isNotEmpty() && !permanentOk(trimmed)) {
            update { it.copy(error = "Use at least 8 characters.") }
            return false
        }
        if (!store.savePermanent(trimmed)) {
            update { it.copy(error = "Couldn't save the permanent password on this phone.") }
            return false
        }
        if (trimmed.isEmpty()) {
            store.approval = "prompt"
            store.unattendedAccess = false
        }
        update {
            it.copy(
                hasPermanent = trimmed.isNotEmpty(),
                approval = store.approval,
                unattendedAccess = store.unattendedAccess && trimmed.isNotEmpty(),
                notice = when {
                    quiet -> it.notice
                    trimmed.isEmpty() -> "Permanent password removed."
                    else -> "Permanent password saved."
                },
            )
        }
        pushPermanent()
        if (trimmed.isEmpty()) {
            command("""{"cmd":"setApproval","mode":"prompt"}""")
            command("""{"cmd":"setUnattendedAccess","on":false}""")
        }
        return true
    }

    /**
     * Unattended access accepts a permanent-password or trusted session and then
     * the system screen-share dialog. Turning it on still requires a permanent
     * password and the accessibility service. That check is not repeated at
     * startup: a process start must not write the saved choice back to off.
     */
    fun setUnattendedAccess(on: Boolean) {
        if (!on) {
            store.unattendedAccess = false
            update { it.copy(unattendedAccess = false) }
            command(org.json.JSONObject().put("cmd", "setUnattendedAccess").put("on", false).toString())
            return
        }
        val ready = store.permanent.isNotEmpty() && PjAccessibilityService.enabled(app)
        if (!ready) {
            AppLog.i("unattended access stays ${store.unattendedAccess}; the password or accessibility service is not ready")
            update { it.copy(unattendedAccess = store.unattendedAccess) }
            return
        }
        store.unattendedAccess = true
        update { it.copy(unattendedAccess = true) }
        command(org.json.JSONObject().put("cmd", "setUnattendedAccess").put("on", true).toString())
    }

    /**
     * Tells the running host the saved switch. Does not write the preference.
     * A session start decides whether auto-accept can run.
     */
    internal fun syncUnattendedAccess() {
        val saved = store.unattendedAccess
        update { it.copy(unattendedAccess = saved) }
        command(JSONObject().put("cmd", "setUnattendedAccess").put("on", saved).put("sync", true).toString())
        AppLog.i("unattended access saved=$saved; startup did not rewrite it")
    }

    fun setApproval(mode: String) {
        val unattended = mode == "unattended"
        if (unattended && store.permanent.isEmpty()) {
            store.approval = "prompt"
            update { it.copy(approval = "prompt", error = "Save a permanent password first.") }
            command("""{"cmd":"setApproval","mode":"prompt"}""")
            return
        }
        val stored = if (unattended) "unattended" else "prompt"
        store.approval = stored
        update { it.copy(approval = stored) }
        command("""{"cmd":"setApproval","mode":"$stored"}""")
    }

    /** The permanent password the host is given. Empty when none is saved. */
    internal fun permanentForHost(): String = store.permanent

    private fun pushPermanent() {
        // The command carries the secret to the host. Do not log this JSON.
        command(JSONObject().put("cmd", "setPermanent").put("password", store.permanent).toString())
    }

    /** Saves the server access key and gives it to the host. Do not log this JSON. */
    fun setAccessKey(value: String) {
        store.accessKey = value.trim()
        pushAccessKey()
    }

    private fun pushAccessKey() {
        command(JSONObject().put("cmd", "setAccessKey").put("key", store.accessKey).toString())
    }

    fun regenerate() = command("""{"cmd":"regenerate"}""")

    fun refreshSupport() = command("""{"cmd":"refreshSupport"}""")
    fun setServer(address: String) = applyAccountServer(address, "", quiet = false)
    fun resetFingerprint() = command("""{"cmd":"resetFingerprint"}""")
    fun captureSize(width: Int, height: Int) =
        command(JSONObject().put("cmd", "capture").put("width", width).put("height", height).toString())

    fun dismissError() = update { it.copy(error = null) }

    fun dismissNotice() = update { it.copy(notice = null) }

    fun shutdown() {
        val h = handle.getAndSet(0L)
        if (h != 0L) Native.nativeDestroy(h)
    }

    override fun onEvent(json: String) {
        val event = runCatching { JSONObject(json) }.getOrNull() ?: return
        main.post { apply(event) }
    }

    override fun onVideo(data: ByteArray, width: Int, height: Int, keyframe: Boolean) {
        val copy = data.copyOf()
        main.post {
            update { state ->
                val session = state.session ?: return@update state
                if (session.frameW == width && session.frameH == height) state
                else state.copy(session = session.copy(frameW = width, frameH = height))
            }
            onVideo?.invoke(copy, width, height, keyframe)
        }
    }

    private fun apply(event: JSONObject) {
        val kind = event.optString("event")
        if (kind == "registered") {
            AppLog.i("event registered id=${event.optString("idFormatted")}")
        } else if (kind != "log") {
            AppLog.i("event $kind ${event.optString("text")} ${event.optString("reason")}")
        }
        when (kind) {
            "log" -> {
                val text = event.optString("text")
                AppLog.i(text)
                if (text.startsWith("sign-in ")) SignInSteps.note(text)
            }
            "release" -> update { it.copy(latestRelease = event.optString("version").ifBlank { it.latestRelease }) }
            "serverLink" -> CloudGate.noteServer(event.optBoolean("up"))
            "account" -> CloudGate.noteAccount(event.toString())
            "registered" -> {
                // The Keystore is the source of truth. A registration that
                // races ahead of setPermanent must not clear a password that
                // was just saved. If the host is behind, tell it once.
                val saved = store.permanent.isNotEmpty()
                if (event.optBoolean("permanent") == saved) {
                    permanentRepush = false
                } else if (!permanentRepush) {
                    permanentRepush = true
                    pushPermanent()
                }
                update {
                    val ready = it.status == "Connecting…" || it.status == "Online" || it.status.isBlank() ||
                    it.status.startsWith("Starting PJDesk")
                    it.copy(
                        id = event.optString("id"),
                        idFormatted = event.optString("idFormatted"),
                        password = event.optString("password"),
                        supportCode = event.optString("supportCode"),
                        supportPassword = event.optString("supportPassword"),
                        supportBorn = event.optLong("supportBorn"),
                        hasPermanent = store.permanent.isNotEmpty(),
                        fingerprint = event.optString("fingerprint"),
                        server = event.optString("server", it.server),
                        approval = store.approval,
                        selfVersion = event.optString("appVersion").ifBlank { it.selfVersion },
                        incomingOnly = if (event.has("incomingOnly")) event.optBoolean("incomingOnly") else it.incomingOnly,
                        status = if (ready) "Ready" else it.status,
                    )
                }
            }
            "status" -> update {
                val text = event.optString("text")
                if (text == "Ready" && (it.hosting || it.incoming != null || it.session != null)) it
                else it.copy(status = text, signalError = if (text == "Ready") "" else it.signalError)
            }
            "signal" -> {
                val via = event.optString("via")
                val reached = event.optString("reached")
                if (via == "LAN" && reached.isNotBlank()) {
                    store.rememberLanPath(NetworkPath.key(app), reached)
                }
                update {
                    it.copy(
                        signalVia = via,
                        signalReached = reached,
                        signalError = event.optString("error"),
                        serverLan = event.optString("lan"),
                        server = event.optString("public").ifBlank { it.server },
                    )
                }
            }
            "openUrl" -> {
                val url = event.optString("url")
                val text = event.optString("text").ifBlank { "Opening the download." }
                update { it.copy(status = text, notice = text) }
                if (url.startsWith("https://")) {
                    val view = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    runCatching { app.startActivity(view) }.onFailure { failure ->
                        update { it.copy(error = "Couldn't open the update: ${failure.message}") }
                    }
                }
            }
            "needPassword" -> update {
                it.copy(
                    askPassword = true,
                    askName = event.optString("name"),
                    askRejected = event.optBoolean("rejected"),
                    askNote = event.optString("message"),
                )
            }
            "clearSaved" -> {
                val id = event.optString("id")
                if (id.isNotBlank()) store.saveComputerPassword(id, "")
            }
            "accountSession" -> noteAccountSession(event.optString("state"))
            "error" -> {
                val text = event.optString("text").trim()
                when (text) {
                    "stopped" -> AppLog.i("connect replaced by a newer attempt")
                    "cancelled" -> update {
                        it.copy(
                            askPassword = false,
                            askNote = "",
                            status = if (it.status.startsWith("Connecting")) "Ready" else it.status,
                        )
                    }
                    else -> update { it.copy(error = text, status = text, askPassword = false, askNote = "") }
                }
            }
            "incoming" -> {
                val name = event.optString("name")
                val view = event.optBoolean("view")
                val bypass = event.optBoolean("bypass")
                val who = name.ifBlank { "Someone" }
                update {
                    it.copy(
                        incoming = IncomingCall(name, event.optString("ip"), view, bypass),
                        status = if (view) "$who wants to view your screen" else "Waiting for approval",
                    )
                }
                if (view) {
                    if (bypass && us.spader.pjdesk.service.HostService.capturing) {
                        accept()
                    } else {
                        us.spader.pjdesk.service.ViewPrompt.show(app, who, bypass)
                    }
                }
            }
            "needCapture" -> {
                us.spader.pjdesk.service.HostService.consentWait(app)
                val saved = store.unattendedAccess
                val passwordReady = store.permanent.isNotEmpty()
                val requested = event.optBoolean("auto") && saved && passwordReady
                if (requested) {
                    AppLog.i("unattended access will accept the screen share dialog")
                }
                if (us.spader.pjdesk.service.HostService.capturing) {
                    if (requested) us.spader.pjdesk.a11y.ProjectionConsent.markAlreadySharing()
                    setCapturing(true)
                } else if (requested) {
                    us.spader.pjdesk.a11y.ProjectionConsent.request(app)
                } else {
                    us.spader.pjdesk.service.HostService.preferAutoClick = false
                    us.spader.pjdesk.service.CapturePrompt.show(app)
                }
            }
            "hosting" -> {
                us.spader.pjdesk.service.ViewPrompt.cancel(app)
                // The visitor card arrives before the picture. The share
                // notification stays until capture is actually running.
                if (us.spader.pjdesk.service.HostService.capturing) {
                    us.spader.pjdesk.service.CapturePrompt.cancel(app)
                    us.spader.pjdesk.a11y.ProjectionConsent.stopWatching()
                }
                val auto = us.spader.pjdesk.a11y.ProjectionConsent.takeAutomatic() || event.optBoolean("auto")
                if (auto) AppLog.i(us.spader.pjdesk.a11y.ProjectionConsent.NOTICE)
                update {
                    val who = event.optString("who").ifBlank { event.optString("peer") }
                    val status = if (auto) us.spader.pjdesk.a11y.ProjectionConsent.NOTICE else "Being controlled by $who"
                    it.copy(
                        hosting = true,
                        incoming = null,
                        hostWho = who,
                        acceptedAutomatically = auto,
                        status = status,
                    )
                }
            }
            "controlling" -> {
                val peer = event.optString("peer")
                us.spader.pjdesk.service.SessionNotice.show(app, peer)
                update {
                val path = event.optString("path")
                val how = when (path) {
                    "LAN" -> "over LAN"
                    "Internet, relayed" -> "through the relay"
                    else -> "($path)"
                }
                    it.copy(
                    askPassword = false,
                    reconnecting = false,
                    reconnectSeconds = 0,
                    session = SessionInfo(
                        peer = peer,
                        id = event.optString("id"),
                        path = path,
                        platform = event.optString("platform"),
                        version = event.optString("version"),
                        layout = event.optBoolean("layout"),
                        displays = it.session?.displays ?: emptyList(),
                    ),
                    status = "Controlling $peer $how",
                )
                }
            }
            "platform" -> update { state ->
                val session = state.session ?: return@update state
                state.copy(session = session.copy(platform = event.optString("platform"), version = event.optString("version")))
            }
            "displays" -> update { state ->
                val session = state.session ?: return@update state
                state.copy(session = session.copy(layout = event.optBoolean("layout"), displays = parseDisplays(event.optJSONArray("displays"))))
            }
            "welcome" -> update { state ->
                val session = state.session ?: return@update state
                val fromWelcome = parseDisplays(event.optJSONArray("displays")).ifEmpty {
                    parseDisplays(event.optJSONArray("monitors"))
                }
                val permissions = if (event.has("permissions")) event.getInt("permissions") else session.permissions
                state.copy(
                    session = session.copy(
                        layout = event.optBoolean("layout"),
                        platform = event.optString("platform", session.platform),
                        displays = fromWelcome.ifEmpty { session.displays },
                        permissions = permissions,
                    ),
                )
            }
            "quality" -> {
                onQuality?.invoke(event.optInt("fps"), event.optInt("bitrate"), event.optInt("maxWidth"))
                update { state ->
                    val session = state.session ?: return@update state
                    state.copy(
                        session = session.copy(
                            fps = event.optInt("fps"),
                            bitrateKbps = event.optInt("bitrate"),
                        ),
                    )
                }
            }
            "pong" -> update { state ->
                val session = state.session ?: return@update state
                val sent = event.optLong("t_us")
                val now = System.nanoTime() / 1_000L
                val ms = if (sent <= 0L) session.latencyMs else ((now - sent) / 1_000L).toInt().coerceIn(0, 60_000)
                state.copy(session = session.copy(latencyMs = ms))
            }
            "clipboard" -> onClipboard?.invoke(event.optString("text"))
            "files" -> {
                val names = event.optJSONArray("names")
                val list = buildList {
                    if (names != null) {
                        for (i in 0 until names.length()) {
                            val name = names.optString(i)
                            if (name.isNotBlank()) add(name)
                        }
                    }
                }
                onFiles?.invoke(list)
            }
            "incomingOnly" -> update { it.copy(incomingOnly = event.optBoolean("on")) }
            "chatReady" -> update { it.copy(chatReady = true) }
            "chatSeen" -> update { it.copy(chatUnread = 0, chatAlert = false) }
            "chat" -> update {
                val line = ChatLine(
                    mine = event.optBoolean("mine"),
                    name = event.optString("name"),
                    text = event.optString("text"),
                    unix = event.optLong("unix"),
                )
                if (line.text.isBlank()) return@update it
                val lines = (it.chatLines + line).takeLast(200)
                val alert = event.optBoolean("alert") && !line.mine
                val unread = if (line.mine || !alert) it.chatUnread else it.chatUnread + 1
                it.copy(chatLines = lines, chatUnread = unread, chatSeq = it.chatSeq + 1, chatAlert = alert)
            }
            "keyframe" -> onKeyframe?.invoke()
            "input" -> {
                if (!RemoteControl.handle(event)) setControl(false)
            }
            "reconnecting" -> update {
                it.copy(
                    reconnecting = true,
                    reconnectSeconds = event.optInt("after"),
                    error = null,
                    status = event.optString("reason").ifBlank { "Connection lost, reconnecting…" },
                )
            }
            "disconnected" -> {
                us.spader.pjdesk.a11y.ProjectionConsent.reset()
                us.spader.pjdesk.service.ViewPrompt.cancel(app)
                us.spader.pjdesk.service.CapturePrompt.cancel(app)
                val role = event.optString("role")
                if (role == "controller" || role == "session") {
                    us.spader.pjdesk.service.SessionNotice.hide(app)
                }
                us.spader.pjdesk.service.ChatNotice.hide(app)
                if (role == "host" || role == "session") {
                    runCatching { us.spader.pjdesk.service.HostService.release(app) }
                }
                val reason = event.optString("reason").ifBlank { "the session ended" }
                update {
                    it.copy(
                        hosting = if (role == "host" || role == "session") false else it.hosting,
                        hostWho = if (role == "host" || role == "session") "" else it.hostWho,
                        acceptedAutomatically = if (role == "host" || role == "session") false else it.acceptedAutomatically,
                        reconnecting = if (role == "controller" || role == "session") false else it.reconnecting,
                        reconnectSeconds = if (role == "controller" || role == "session") 0 else it.reconnectSeconds,
                        session = if (role == "controller" || role == "session") null else it.session,
                        chatReady = false,
                        chatLines = emptyList(),
                        chatUnread = 0,
                        chatAlert = false,
                        incoming = null,
                        status = "Ended: $reason",
                    )
                }
            }
        }
    }

    private fun parseDisplays(array: JSONArray?): List<DisplayInfo> {
        if (array == null) return emptyList()
        return buildList {
            for (i in 0 until array.length()) {
                val o = array.optJSONObject(i) ?: continue
                add(
                    DisplayInfo(
                        index = o.optInt("index"),
                        name = o.optString("name"),
                        width = o.optInt("width"),
                        height = o.optInt("height"),
                        primary = o.optBoolean("primary"),
                    ),
                )
            }
        }
    }

    private fun deviceName(context: Context): String {
        val named = runCatching {
            Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
        }.getOrNull()?.trim().orEmpty()
        if (named.isNotEmpty()) return named.take(64)
        return (Build.MODEL ?: "Android").take(64)
    }

    /** The Android user profile, when the system allows it. Blank otherwise. */
    private fun profileName(context: Context): String {
        return try {
            val manager = context.getSystemService(UserManager::class.java) ?: return ""
            val method = UserManager::class.java.getMethod("getUserName")
            (method.invoke(manager) as? String)?.trim()?.take(80).orEmpty()
        } catch (_: Throwable) {
            ""
        }
    }

    private fun update(block: (DeskState) -> DeskState) {
        _state.value = block(_state.value)
    }
}
