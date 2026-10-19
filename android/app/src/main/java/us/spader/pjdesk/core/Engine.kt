package us.spader.pjdesk.core

import android.content.Context
import android.os.Handler
import android.os.Looper
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

    private val _state = MutableStateFlow(DeskState(approval = store.approval))
    val state: StateFlow<DeskState> = _state.asStateFlow()

    var onVideo: ((ByteArray, Int, Int, Boolean) -> Unit)? = null
    var onQuality: ((Int, Int, Int) -> Unit)? = null
    var onKeyframe: (() -> Unit)? = null

    fun start() {
        if (!Native.loaded || handle.get() != 0L) return
        val androidId = Settings.Secure.getString(app.contentResolver, Settings.Secure.ANDROID_ID) ?: ""
        val host = android.os.Build.MODEL ?: "Android"
        val created = Native.nativeCreate(app.filesDir.absolutePath, androidId, host, this)
        handle.set(created)
        if (created == 0L) {
            update { it.copy(status = "The PJDesk engine did not start.", error = "nativeCreate returned 0") }
            return
        }
        command("""{"cmd":"setApproval","mode":"${store.approval}"}""")
        if (store.permanent.isNotEmpty()) {
            command(JSONObject().put("cmd", "setPermanent").put("password", store.permanent).toString())
        }
    }

    fun command(json: String) {
        val h = handle.get()
        if (h != 0L) Native.nativeCommand(h, json)
    }

    fun submitVideo(data: ByteArray, width: Int, height: Int, keyframe: Boolean, encodeUs: Int) {
        val h = handle.get()
        if (h != 0L) Native.nativeSubmitVideo(h, data, width, height, keyframe, encodeUs)
    }

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

    fun setPermanent(password: String) {
        store.permanent = password
        command(JSONObject().put("cmd", "setPermanent").put("password", password).toString())
    }

    fun setApproval(mode: String) {
        store.approval = mode
        command(JSONObject().put("cmd", "setApproval").put("mode", mode).toString())
    }

    fun regenerate() = command("""{"cmd":"regenerate"}""")
    fun setServer(address: String) = command(JSONObject().put("cmd", "server").put("address", address).toString())
    fun resetFingerprint() = command("""{"cmd":"resetFingerprint"}""")
    fun captureSize(width: Int, height: Int) =
        command(JSONObject().put("cmd", "capture").put("width", width).put("height", height).toString())

    fun dismissError() = update { it.copy(error = null) }

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
        if (kind != "log") AppLog.i("event $kind ${event.optString("text")} ${event.optString("reason")}")
        when (kind) {
            "log" -> AppLog.i(event.optString("text"))
            "registered" -> update {
                it.copy(
                    id = event.optString("id"),
                    idFormatted = event.optString("idFormatted"),
                    password = event.optString("password"),
                    hasPermanent = event.optBoolean("permanent"),
                    fingerprint = event.optString("fingerprint"),
                    server = event.optString("server", it.server),
                    approval = event.optString("approval", it.approval),
                    status = if (it.status == "Connecting…") "Online" else it.status,
                )
            }
            "status" -> update { it.copy(status = event.optString("text")) }
            "error" -> update { it.copy(error = event.optString("text")) }
            "incoming" -> update {
                it.copy(incoming = IncomingCall(event.optString("name"), event.optString("ip")))
            }
            "hosting" -> update { it.copy(hosting = true, incoming = null, status = "Being controlled by ${event.optString("peer")}") }
            "controlling" -> update {
                it.copy(
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
                val role = event.optString("role")
                update {
                    it.copy(
                        hosting = if (role == "host" || role == "session") false else it.hosting,
                        session = if (role == "controller" || role == "session") null else it.session,
                        incoming = null,
                        status = event.optString("reason").ifBlank { "Disconnected" },
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

    private fun update(block: (DeskState) -> DeskState) {
        _state.value = block(_state.value)
    }
}
