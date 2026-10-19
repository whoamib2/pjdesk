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
import org.json.JSONArray
import org.json.JSONObject
import us.spader.pjdesk.AppLog
import us.spader.pjdesk.data.Store
import us.spader.pjdesk.input.GestureMap
import us.spader.pjdesk.service.RemoteControl
import java.util.concurrent.atomic.AtomicLong

class Engine(context: Context) : EngineListener {
    private val app = context.applicationContext
    private val store = Store(app)
    private val main = Handler(Looper.getMainLooper())
    private val handle = AtomicLong(0)
    /** One retry when a registration event disagrees with the Keystore, so a stale event cannot loop. */
    private var permanentRepush = false

    private val _state = MutableStateFlow(
        DeskState(
            approval = store.approval,
            hasPermanent = store.permanent.isNotEmpty(),
        ),
    )
    val state: StateFlow<DeskState> = _state.asStateFlow()

    var onVideo: ((ByteArray, Int, Int, Boolean) -> Unit)? = null
    var onQuality: ((Int, Int, Int) -> Unit)? = null
    var onKeyframe: (() -> Unit)? = null

    fun start() {
        if (!Native.loaded || handle.get() != 0L) return
        val androidId = Settings.Secure.getString(app.contentResolver, Settings.Secure.ANDROID_ID) ?: ""
        val host = deviceName(app)
        val created = Native.nativeCreate(app.filesDir.absolutePath, androidId, host, this)
        handle.set(created)
        if (created == 0L) {
            update { it.copy(status = "The PJDesk engine did not start.", error = "nativeCreate returned 0") }
            return
        }
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
        command("""{"cmd":"setApproval","mode":"${store.approval}"}""")
        pushTrusted()
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

    fun command(json: String) {
        val h = handle.get()
        if (h != 0L) Native.nativeCommand(h, json)
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

    fun connect(id: String, password: String) = command(
        JSONObject().put("cmd", "connect").put("id", id).put("password", password).toString(),
    )

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

    fun mouse(kind: String, x: Int, y: Int, dy: Int = 0) {
        command(JSONObject().put("cmd", "mouse").put("kind", kind).put("x", x).put("y", y).put("dy", dy).toString())
    }

    fun text(chars: String) = command(JSONObject().put("cmd", "text").put("chars", chars).toString())
    fun special(which: Int) = command(JSONObject().put("cmd", "special").put("which", which).toString())
    fun key(scan: Int, down: Boolean) = command(JSONObject().put("cmd", "key").put("scan", scan).put("down", down).toString())
    fun releaseKeys() = command("""{"cmd":"release"}""")
    fun quality(name: String) = command(JSONObject().put("cmd", "quality").put("name", name).toString())
    fun display(index: Int, layout: Boolean) =
        command(JSONObject().put("cmd", "display").put("index", index).put("layout", layout).toString())

    fun keyframe() = command("""{"cmd":"keyframe"}""")

    /**
     * Saves [password] in the Keystore and gives it to the running host.
     * An empty password removes it. Returns false when it could not be stored.
     * The password itself is never written to the log.
     */
    fun setPermanent(password: String): Boolean {
        val trimmed = password.trim()
        if (trimmed.isNotEmpty() && !permanentOk(trimmed)) {
            update { it.copy(error = "Use at least 8 characters.") }
            return false
        }
        if (!store.savePermanent(trimmed)) {
            update { it.copy(error = "Couldn't save the permanent password on this phone.") }
            return false
        }
        if (trimmed.isEmpty()) store.approval = "prompt"
        update {
            it.copy(
                hasPermanent = trimmed.isNotEmpty(),
                approval = store.approval,
                notice = if (trimmed.isEmpty()) "Permanent password removed." else "Permanent password saved.",
            )
        }
        pushPermanent()
        if (trimmed.isEmpty()) command("""{"cmd":"setApproval","mode":"prompt"}""")
        return true
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

    fun regenerate() = command("""{"cmd":"regenerate"}""")
    fun setServer(address: String) = command(JSONObject().put("cmd", "server").put("address", address).toString())
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
            "log" -> AppLog.i(event.optString("text"))
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
                    val ready = it.status == "Connecting…" || it.status == "Online" || it.status.isBlank()
                    it.copy(
                        id = event.optString("id"),
                        idFormatted = event.optString("idFormatted"),
                        password = event.optString("password"),
                        hasPermanent = store.permanent.isNotEmpty(),
                        fingerprint = event.optString("fingerprint"),
                        server = event.optString("server", it.server),
                        approval = store.approval,
                        status = if (ready) "Ready" else it.status,
                    )
                }
            }
            "status" -> update {
                val text = event.optString("text")
                if (text == "Ready" && (it.hosting || it.incoming != null || it.session != null)) it
                else it.copy(status = text)
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
            "error" -> update {
                val text = event.optString("text")
                it.copy(error = text, status = text, askPassword = false, askNote = "")
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
            "hosting" -> {
                us.spader.pjdesk.service.ViewPrompt.cancel(app)
                update {
                    val who = event.optString("who").ifBlank { event.optString("peer") }
                    it.copy(hosting = true, incoming = null, hostWho = who, status = "Being controlled by $who")
                }
            }
            "controlling" -> update {
                it.copy(
                    askPassword = false,
                    session = SessionInfo(
                        peer = event.optString("peer"),
                        id = event.optString("id"),
                        path = event.optString("path"),
                        platform = event.optString("platform"),
                        version = event.optString("version"),
                        layout = event.optBoolean("layout"),
                        displays = it.session?.displays ?: emptyList(),
                    ),
                    status = "Controlling ${event.optString("peer")}",
                )
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
                state.copy(
                    session = session.copy(
                        layout = event.optBoolean("layout"),
                        platform = event.optString("platform", session.platform),
                        displays = fromWelcome.ifEmpty { session.displays },
                    ),
                )
            }
            "quality" -> onQuality?.invoke(event.optInt("fps"), event.optInt("bitrate"), event.optInt("maxWidth"))
            "keyframe" -> onKeyframe?.invoke()
            "input" -> RemoteControl.handle(event)
            "disconnected" -> {
                us.spader.pjdesk.service.ViewPrompt.cancel(app)
                val role = event.optString("role")
                val reason = event.optString("reason").ifBlank { "the session ended" }
                update {
                    it.copy(
                        hosting = if (role == "host" || role == "session") false else it.hosting,
                        hostWho = if (role == "host" || role == "session") "" else it.hostWho,
                        session = if (role == "controller" || role == "session") null else it.session,
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
