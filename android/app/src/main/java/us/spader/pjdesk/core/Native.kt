package us.spader.pjdesk.core

import android.util.Log

/**
 * JNI bindings for `libpjdesk_mobile`. The Rust side is
 * `crates/pjdesk-mobile/src/jni.rs`.
 */
object Native {
    private const val TAG = "pjdesk"

    @Volatile
    var loaded: Boolean = false
        private set

    init {
        try {
            System.loadLibrary("pjdesk_mobile")
            loaded = true
        } catch (e: UnsatisfiedLinkError) {
            Log.e(TAG, "libpjdesk_mobile is missing. Run android/build.sh.", e)
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
