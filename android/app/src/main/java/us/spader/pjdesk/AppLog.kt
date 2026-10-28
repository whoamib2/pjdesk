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
    /** Stable logcat tag for the last Connect failure: `adb logcat -s PJDeskConnect`. */
    const val CONNECT_TAG = "PJDeskConnect"
    private const val MAX_BYTES = 512 * 1024
    private lateinit var file: File
    private lateinit var signInFile: File

    fun init(context: Context) {
        val dir = File(context.applicationContext.filesDir, "logs").apply { mkdirs() }
        file = File(dir, "pjdesk.log")
        signInFile = File(dir, "signin.log")
        i("PJDesk log opened")
    }

    fun i(message: String) {
        // A release build on current Android drops INFO. logd's isLoggable check
        // and R8's Log.i removal both do that. WARN is the lowest priority those
        // devices still keep. ERROR is used when WARN is refused too. The file
        // keeps the I mark, and Share logs sends the file.
        write("I", message)
        if (message.contains("sign-in ")) appendSignIn(message)
        if (Log.isLoggable(TAG, Log.WARN)) Log.w(TAG, message) else Log.e(TAG, message)
    }

    fun e(message: String, error: Throwable? = null) {
        val detail = error?.message?.let { ": $it" }.orEmpty()
        write("E", message + detail)
        if (message.startsWith("connect failed")) noteConnectError(message.removePrefix("connect failed:").trim())
        Log.e(TAG, message, error)
    }

    /**
     * The last Connect failure, in the app's private files. One short line.
     * It is overwritten, so it is still there after logcat rotates.
     * Passwords, tokens, and keys are not written.
     */
    fun noteConnectError(message: String) {
        if (!::file.isInitialized) return
        val text = us.spader.pjdesk.core.redactConnectError(message).trim()
        if (text.isEmpty()) return
        // ERROR priority, so a release build keeps it, under its own tag so it
        // is easy to find among the main PJDesk lines.
        Log.e(CONNECT_TAG, text)
        val dest = File(file.parentFile, "last-connect.txt")
        runCatching { dest.writeText("${Instant.now()} $text\n") }
    }

    /** The saved last Connect failure, already redacted, or empty. */
    fun lastConnectError(): String {
        if (!::file.isInitialized) return ""
        val src = File(file.parentFile, "last-connect.txt")
        if (!src.exists()) return ""
        val text = runCatching { src.readText() }.getOrDefault("")
        return us.spader.pjdesk.core.redactConnectError(text).trim()
    }

    /** Shares the last Connect failure as plain text. Nothing is sent without the person choosing an app. */
    fun shareLastError(context: Context) {
        val text = lastConnectError().ifEmpty { "No Connect error has been saved on this phone." }
        val version = runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }
            .getOrNull()
            .orEmpty()
        val send = Intent(Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(Intent.EXTRA_SUBJECT, "PJDesk last connect error")
            putExtra(Intent.EXTRA_TEXT, "PJDesk $version\n$text".trim())
        }
        context.startActivity(Intent.createChooser(send, "Share last error").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    fun logFile(): File? = if (::file.isInitialized && file.exists()) file else null

    /** Sign-in lines kept in the app log, including the previous file after a roll. */
    fun recentSignIn(limit: Int = 50): String {
        if (::signInFile.isInitialized && signInFile.exists()) {
            val text = runCatching { signInFile.readText() }.getOrDefault("")
            val lines = us.spader.pjdesk.core.signInLinesFromLog(text, limit)
            if (lines.isNotBlank()) return lines
        }
        if (!::file.isInitialized) return ""
        val older = File(file.parentFile, "pjdesk.log.1")
        val text = buildString {
            if (older.exists()) append(runCatching { older.readText() }.getOrDefault(""))
            if (isNotEmpty()) append('\n')
            if (file.exists()) append(runCatching { file.readText() }.getOrDefault(""))
        }
        return us.spader.pjdesk.core.signInLinesFromLog(text, limit)
    }

    @Synchronized
    private fun appendSignIn(message: String) {
        if (!::signInFile.isInitialized) return
        runCatching {
            if (signInFile.length() > 64 * 1024) {
                val previous = File(signInFile.parentFile, "signin.log.1")
                if (previous.exists()) previous.delete()
                signInFile.renameTo(previous)
            }
            signInFile.appendText("${Instant.now()} $message\n")
        }
    }

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
