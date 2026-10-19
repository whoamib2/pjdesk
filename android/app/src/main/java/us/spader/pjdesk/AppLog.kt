package us.spader.pjdesk

import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.core.content.FileProvider
import java.io.File
import java.time.Instant

/**
 * Logcat tag `PJDesk`, plus a rolling file under the app's private storage.
 * Share logs sends that file. Nothing here is uploaded on its own.
 */
object AppLog {
    const val TAG = "PJDesk"
    private const val MAX_BYTES = 512 * 1024
    private lateinit var file: File

    fun init(context: Context) {
        val dir = File(context.applicationContext.filesDir, "logs").apply { mkdirs() }
        file = File(dir, "pjdesk.log")
        i("PJDesk log opened")
    }

    fun i(message: String) {
        Log.i(TAG, message)
        write("I", message)
    }

    fun e(message: String, error: Throwable? = null) {
        Log.e(TAG, message, error)
        val detail = error?.message?.let { ": $it" }.orEmpty()
        write("E", message + detail)
    }

    fun logFile(): File? = if (::file.isInitialized && file.exists()) file else null

    fun share(context: Context) {
        val log = logFile()
        if (log == null) {
            i("Share logs: no file yet")
            return
        }
        val uri = FileProvider.getUriForFile(context, "us.spader.pjdesk.files", log)
        val send = Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_STREAM, uri)
            putExtra(Intent.EXTRA_SUBJECT, "PJDesk logs")
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        }
        context.startActivity(Intent.createChooser(send, "Share PJDesk logs").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    @Synchronized
    private fun write(level: String, message: String) {
        if (!::file.isInitialized) return
        runCatching {
            if (file.length() > MAX_BYTES) {
                val previous = File(file.parentFile, "pjdesk.log.1")
                if (previous.exists()) previous.delete()
                file.renameTo(previous)
            }
            file.appendText("${Instant.now()} $level $message\n")
        }
    }
}
