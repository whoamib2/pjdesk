package us.spader.pjdesk.core

import org.json.JSONObject

/**
 * Remote-control defaults. A session can store its own copy for one computer.
 * The three older keys (pointer speed, acceleration, idle minutes) stay in
 * their own preferences so a phone that already saved them keeps those values.
 */
data class RemoteOptions(
    val inputMode: String = "auto",
    val pointerSpeed: Float = 0.6f,
    val pointerAccel: Boolean = false,
    val scrollSpeed: Float = 1f,
    val invertScroll: Boolean = false,
    val dragLock: Boolean = false,
    val showLocalCursor: Boolean = true,
    val scale: String = "fit",
    val quality: String = "balanced",
    val customFps: Int = 30,
    val customBitrate: Int = 6000,
    val viewOnly: Boolean = false,
    val showStats: Boolean = false,
    val privacy: Boolean = false,
    val showKeyBar: Boolean = false,
    val keyboardMode: String = "translate",
    val clipboardSync: Boolean = false,
    val sessionIdleMin: Int = 1,
) {
    /** Auto follows the remote: a phone is touch, anything else is the trackpad. */
    fun trackpadFor(androidRemote: Boolean): Boolean = when (inputMode) {
        "touch" -> false
        "trackpad" -> true
        else -> !androidRemote
    }

    fun cleaned(): RemoteOptions = copy(
        inputMode = if (inputMode == "touch" || inputMode == "trackpad") inputMode else "auto",
        pointerSpeed = pointerSpeed.coerceIn(0.25f, 2f),
        scrollSpeed = scrollSpeed.coerceIn(0.25f, 4f),
        scale = if (scale == "original" || scale == "stretch") scale else "fit",
        quality = if (quality == "best" || quality == "speed" || quality == "custom") quality else "balanced",
        customFps = customFps.coerceIn(5, 60),
        customBitrate = customBitrate.coerceIn(200, 20_000),
        keyboardMode = if (keyboardMode == "map") "map" else "translate",
        sessionIdleMin = if (sessionIdleMin == 0 || sessionIdleMin == 1 || sessionIdleMin == 5 || sessionIdleMin == 15) {
            sessionIdleMin
        } else {
            1
        },
    )

    fun toJson(): JSONObject {
        val o = cleaned()
        return JSONObject()
            .put("inputMode", o.inputMode)
            .put("pointerSpeed", o.pointerSpeed.toDouble())
            .put("pointerAccel", o.pointerAccel)
            .put("scrollSpeed", o.scrollSpeed.toDouble())
            .put("invertScroll", o.invertScroll)
            .put("dragLock", o.dragLock)
            .put("showLocalCursor", o.showLocalCursor)
            .put("scale", o.scale)
            .put("quality", o.quality)
            .put("customFps", o.customFps)
            .put("customBitrate", o.customBitrate)
            .put("viewOnly", o.viewOnly)
            .put("showStats", o.showStats)
            .put("privacy", o.privacy)
            .put("showKeyBar", o.showKeyBar)
            .put("keyboardMode", o.keyboardMode)
            .put("clipboardSync", o.clipboardSync)
            .put("sessionIdleMin", o.sessionIdleMin)
    }

    companion object {
        const val PERM_CLIPBOARD = 1 shl 2
        const val PERM_FILE = 1 shl 3
        const val PERM_AUDIO = 1 shl 4
        const val PERM_RESTART = 1 shl 5
        const val PERM_PRIVACY = 1 shl 6

        /** Unknown permissions (`< 0`) leave the control available. The host can still refuse. */
        fun allows(permissions: Int, bit: Int): Boolean = permissions < 0 || permissions and bit == bit

        /** Names the Windows presets already understand. */
        fun preset(quality: String): String = when (quality) {
            "best" -> "high"
            "speed" -> "smooth"
            else -> "balanced"
        }

        fun fromJson(raw: String?): RemoteOptions {
            if (raw.isNullOrBlank()) return RemoteOptions()
            val o = runCatching { JSONObject(raw) }.getOrElse { return RemoteOptions() }
            return RemoteOptions(
                inputMode = o.optString("inputMode", "auto"),
                pointerSpeed = o.optDouble("pointerSpeed", 0.6).toFloat(),
                pointerAccel = o.optBoolean("pointerAccel", false),
                scrollSpeed = o.optDouble("scrollSpeed", 1.0).toFloat(),
                invertScroll = o.optBoolean("invertScroll", false),
                dragLock = o.optBoolean("dragLock", false),
                showLocalCursor = o.optBoolean("showLocalCursor", true),
                scale = o.optString("scale", "fit"),
                quality = o.optString("quality", "balanced"),
                customFps = o.optInt("customFps", 30),
                customBitrate = o.optInt("customBitrate", 6000),
                viewOnly = o.optBoolean("viewOnly", false),
                showStats = o.optBoolean("showStats", false),
                privacy = o.optBoolean("privacy", false),
                showKeyBar = o.optBoolean("showKeyBar", false),
                keyboardMode = o.optString("keyboardMode", "translate"),
                clipboardSync = o.optBoolean("clipboardSync", false),
                sessionIdleMin = o.optInt("sessionIdleMin", 1),
            ).cleaned()
        }
    }
}
