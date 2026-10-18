package us.spader.pjdesk.media

import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.view.Surface

/**
 * Decodes the same Annex-B H.264 access units the Windows client produces
 * with OpenH264, onto a [Surface].
 */
class ScreenDecoder(private val onSize: (Int, Int) -> Unit) {
    private val thread = HandlerThread("pjdesk-decode").also { it.start() }
    private val handler = Handler(thread.looper)
    private var codec: MediaCodec? = null
    private var surface: Surface? = null
    private var width = 0
    private var height = 0

    fun setSurface(surface: Surface?) {
        handler.post {
            this.surface = surface
            releaseCodec()
        }
    }

    fun submit(data: ByteArray, w: Int, h: Int, keyframe: Boolean) {
        val copy = data.copyOf()
        handler.post { feed(copy, w, h, keyframe) }
    }

    fun stop() {
        handler.post {
            releaseCodec()
            thread.quitSafely()
        }
    }

    private fun feed(data: ByteArray, w: Int, h: Int, keyframe: Boolean) {
        val target = surface ?: return
        if (!target.isValid) return
        if (codec == null || w != width || h != height) {
            if (!keyframe) return
            width = w
            height = h
            releaseCodec()
            val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, w, h)
            val sets = AnnexB.parameterSets(data)
            if (sets.isNotEmpty()) format.setByteBuffer("csd-0", java.nio.ByteBuffer.wrap(sets))
            val decoder = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
            decoder.configure(format, target, null, 0)
            decoder.start()
            codec = decoder
            onSize(w, h)
        }
        val decoder = codec ?: return
        val index = try {
            decoder.dequeueInputBuffer(10_000)
        } catch (e: Exception) {
            Log.w(TAG, "decoder input closed", e)
            releaseCodec()
            return
        }
        if (index >= 0) {
            val buffer = decoder.getInputBuffer(index) ?: return
            buffer.clear()
            buffer.put(data)
            val flags = if (keyframe) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0
            decoder.queueInputBuffer(index, 0, data.size, System.nanoTime() / 1000, flags)
        }
        val info = MediaCodec.BufferInfo()
        while (true) {
            val out = try {
                decoder.dequeueOutputBuffer(info, 0)
            } catch (e: Exception) {
                releaseCodec()
                return
            }
            if (out < 0) break
            decoder.releaseOutputBuffer(out, true)
        }
    }

    private fun releaseCodec() {
        runCatching { codec?.stop() }
        runCatching { codec?.release() }
        codec = null
    }

    companion object {
        private const val TAG = "pjdesk"
    }
}
