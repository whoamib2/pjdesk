package us.spader.pjdesk.media

import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.util.Log
import android.view.Surface
import java.nio.ByteBuffer

/**
 * Hardware H.264 encoder. Output is Annex-B, with SPS and PPS repeated on
 * every keyframe, which is what the Windows OpenH264 decoder already reads.
 *
 * Constrained Baseline is requested when the device lists it. If the encoder
 * rejects that profile, the same call is retried with the device default,
 * which is still `video/avc`.
 */
class ScreenEncoder(
    private val projection: MediaProjection,
    private val width: Int,
    private val height: Int,
    private val dpi: Int,
    private val onFrame: (ByteArray, Int, Int, Boolean, Int) -> Unit,
) {
    private val thread = HandlerThread("pjdesk-encode").also { it.start() }
    private val handler = Handler(thread.looper)
    private val main = Handler(Looper.getMainLooper())
    private var codec: MediaCodec? = null
    private var display: VirtualDisplay? = null
    private var surface: Surface? = null
    private val annex = AnnexB.Converter()
    private var fps = 30
    private var bitrate = 6_000_000
    @Volatile private var running = true

    /**
     * The codec starts on the encoder thread. The virtual display is created
     * back on the main thread, which is where the projection callback is
     * registered. Creating it on the encoder thread makes Android 14+ stop
     * the projection immediately.
     */
    fun start(onBeforeDisplay: () -> Boolean, onReady: () -> Unit, onFailed: (String) -> Unit) {
        handler.post {
            try {
                val encoder = openEncoder()
                codec = encoder
                val input = encoder.createInputSurface()
                surface = input
                encoder.start()
                main.post {
                    try {
                        if (!onBeforeDisplay()) {
                            onFailed("Could not start the screen-capture service")
                            return@post
                        }
                        display = projection.createVirtualDisplay(
                            "pjdesk",
                            width,
                            height,
                            dpi.coerceAtLeast(1),
                            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
                            input,
                            null,
                            main,
                        )
                        if (display == null) {
                            onFailed("Screen capture did not start")
                            return@post
                        }
                        Log.i(TAG, "encoder started ${width}x$height")
                        onReady()
                        handler.post { drain() }
                    } catch (e: Exception) {
                        Log.e(TAG, "virtual display failed", e)
                        onFailed(e.message ?: "Screen capture failed")
                    }
                }
            } catch (e: Exception) {
                Log.e(TAG, "encoder failed", e)
                main.post { onFailed(e.message ?: "Screen capture failed") }
            }
        }
    }

    fun applyQuality(fps: Int, bitrateKbps: Int) {
        this.fps = fps.coerceIn(1, 60)
        bitrate = bitrateKbps.coerceIn(200, 50_000) * 1000
        val bundle = Bundle()
        bundle.putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, bitrate)
        handler.post { runCatching { codec?.setParameters(bundle) } }
    }

    fun requestKeyframe() {
        val bundle = Bundle()
        bundle.putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0)
        handler.post { runCatching { codec?.setParameters(bundle) } }
    }

    fun stop() {
        running = false
        handler.post {
            runCatching { display?.release() }
            runCatching { codec?.stop() }
            runCatching { codec?.release() }
            runCatching { surface?.release() }
            display = null
            codec = null
            surface = null
            thread.quitSafely()
        }
    }

    private fun openEncoder(): MediaCodec {
        val encoder = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        val constrained = format(constrained = true)
        try {
            encoder.configure(constrained, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            return encoder
        } catch (e: Exception) {
            Log.w(TAG, "Constrained Baseline was refused; using the device AVC profile", e)
            encoder.configure(format(constrained = false), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            return encoder
        }
    }

    private fun format(constrained: Boolean): MediaFormat {
        val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, width, height)
        format.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
        format.setInteger(MediaFormat.KEY_BIT_RATE, bitrate)
        format.setInteger(MediaFormat.KEY_FRAME_RATE, fps)
        format.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 2)
        format.setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            format.setInteger(MediaFormat.KEY_LATENCY, 0)
            format.setInteger(MediaFormat.KEY_PRIORITY, 0)
        }
        if (constrained) {
            format.setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileConstrainedBaseline)
            format.setInteger(MediaFormat.KEY_LEVEL, MediaCodecInfo.CodecProfileLevel.AVCLevel31)
        }
        return format
    }

    private fun drain() {
        val buffer = MediaCodec.BufferInfo()
        while (running) {
            val encoder = codec ?: break
            val index = try {
                encoder.dequeueOutputBuffer(buffer, 20_000)
            } catch (e: Exception) {
                Log.w(TAG, "encoder stopped", e)
                break
            }
            when {
                index == MediaCodec.INFO_TRY_AGAIN_LATER -> continue
                index == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                    val csd = encoder.outputFormat.getByteBuffer("csd-0")
                    if (csd != null) annex.config(copy(csd))
                    val csd1 = encoder.outputFormat.getByteBuffer("csd-1")
                    if (csd1 != null) annex.add(copy(csd1))
                }
                index >= 0 -> {
                    val encoded = encoder.getOutputBuffer(index)
                    if (encoded != null && buffer.size > 0 && buffer.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0) {
                        val bytes = ByteArray(buffer.size)
                        encoded.position(buffer.offset)
                        encoded.limit(buffer.offset + buffer.size)
                        encoded.get(bytes)
                        val key = buffer.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
                        val started = System.nanoTime()
                        val access = annex.frame(bytes, key)
                        val us = ((System.nanoTime() - started) / 1000L).toInt().coerceAtLeast(0)
                        onFrame(access, width, height, key, us)
                    } else if (encoded != null && buffer.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                        val bytes = ByteArray(buffer.size)
                        encoded.position(buffer.offset)
                        encoded.limit(buffer.offset + buffer.size)
                        encoded.get(bytes)
                        annex.config(bytes)
                    }
                    encoder.releaseOutputBuffer(index, false)
                }
            }
        }
    }

    private fun copy(buffer: ByteBuffer): ByteArray {
        val dup = buffer.duplicate()
        val bytes = ByteArray(dup.remaining())
        dup.get(bytes)
        return bytes
    }

    companion object {
        private const val TAG = "PJDesk"

        /** Even dimensions, capped by the quality preset's max width. */
        fun fit(srcW: Int, srcH: Int, maxWidth: Int): Pair<Int, Int> {
            if (srcW <= 1 || srcH <= 1) return 2 to 2
            val cap = maxWidth.coerceIn(640, 3840)
            val w = minOf(srcW, cap).let { it - it % 2 }.coerceAtLeast(2)
            val h = ((srcH.toLong() * w) / srcW).toInt().let { it - it % 2 }.coerceAtLeast(2)
            return w to h
        }
    }
}
