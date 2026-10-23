package us.spader.pjdesk.core

import android.util.Log

/**
 * JNI bindings for `libpjdesk_mobile`. The Rust side is
 * `crates/pjdesk-mobile/src/jni.rs`.
 */
/** Shown when `libpjdesk_mobile.so` was not packaged. `assembleRelease` does not compile it. */
const val ENGINE_LIBRARY_MISSING = "Engine library missing from this build"

object Native {
    private const val TAG = "pjdesk"

    @Volatile
    var loaded: Boolean = false
        private set

    /** Set when the native library did not load. Empty once it has loaded. */
    @Volatile
    var loadError: String = ""
        private set

    init {
        try {
            System.loadLibrary("pjdesk_mobile")
            loaded = true
            loadError = ""
        } catch (e: UnsatisfiedLinkError) {
            loadError = ENGINE_LIBRARY_MISSING
            Log.e(TAG, "$ENGINE_LIBRARY_MISSING. Build it with cargo-ndk before assembleRelease. See android/BUILD-WINDOWS.md.", e)
        }
    }

    external fun nativeCreate(dir: String, androidId: String, hostname: String, listener: EngineListener): Long
    external fun nativeDestroy(handle: Long)
    external fun nativeCommand(handle: Long, json: String)
    external fun nativeSubmitVideo(handle: Long, data: ByteArray, width: Int, height: Int, keyframe: Boolean, encodeUs: Int)
    external fun nativeViewToNorm(frameW: Int, frameH: Int, viewW: Int, viewH: Int, x: Float, y: Float): Long
    external fun nativeLetterbox(frameW: Int, frameH: Int, viewW: Int, viewH: Int): IntArray?
}

interface EngineListener {
    fun onEvent(json: String)
    fun onVideo(data: ByteArray, width: Int, height: Int, keyframe: Boolean)
}
