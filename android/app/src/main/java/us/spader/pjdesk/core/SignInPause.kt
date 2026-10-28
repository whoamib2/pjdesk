package us.spader.pjdesk.core

/** The account login window when pjdesk.com does not say how long to wait. */
const val SIGN_IN_PAUSE_SECS = 15 * 60L

/**
 * Seconds to wait after HTTP 429 from `/api/login`. Null for any other failure.
 * `retryAfter` is the Worker's Retry-After delta-seconds and wins over the body.
 * A code-guess or PIN lock ("Too many tries") is not this pause.
 */
fun signInPauseSeconds(status: Int, message: String, retryAfter: Long? = null): Long? {
    if (message.contains("temporarily unavailable") || message.contains("temporarily rate limited")) return null
    val limited = (status == 429 && !message.contains("Too many tries"))
        || message.contains("Too many failed sign-ins")
        || message.contains("Too many sign-in attempts")
    if (!limited) return null
    val header = retryAfter?.takeIf { it > 0 }
    if (header != null) return header.coerceIn(1L, SIGN_IN_PAUSE_SECS)
    return (waitMinutes(message) ?: (SIGN_IN_PAUSE_SECS / 60)) * 60
}

/** A standby miss still lets the phone ask pjdesk.com. A wrong password does not. */
fun standbyMissFallsThrough(message: String): Boolean {
    if (message.isBlank()) return true
    if (serverHasNoAccountCopy(message)) return true
    val lower = message.lowercase()
    if (lower.contains("wrong email") || lower.contains("too many")) return false
    return lower.contains("without a fingerprint") ||
        lower.contains("could not connect") ||
        lower.contains("step=") ||
        lower.contains("timed out") ||
        lower.contains("not saved")
}

/** True when this server answered but could not check the account password. */
fun serverHasNoAccountCopy(message: String): Boolean {
    val lower = message.lowercase()
    return lower.contains("does not have the account") ||
        lower.contains("account copy is empty") ||
        lower.contains("server sign-in needed") ||
        lower.contains("pjdesk.com did not answer") ||
        lower.contains("no password check") ||
        lower.contains("not signed in")
}

/** One PJDesk logcat line. Address, step, and reply. No password or token. */
fun signInLogLine(address: String, step: String, detail: String): String {
    val where = address.trim().ifBlank { "none" }
    val extra = detail.trim()
    return if (extra.isEmpty()) "sign-in address=$where step=$step" else "sign-in address=$where step=$step $extra"
}

/**
 * The last sign-in steps. The Sign in button records the first line before any
 * network call. Copy diagnostics sends this text. The password is not stored.
 */
open class SignInTrace(private val limit: Int = 50) {
    private val lines = ArrayDeque<String>()

    fun note(line: String) {
        val text = line.trim()
        if (text.isEmpty()) return
        synchronized(lines) {
            lines.addLast(text)
            while (lines.size > limit) lines.removeFirst()
        }
    }

    fun snapshot(): List<String> = synchronized(lines) { lines.toList() }

    fun copyText(): String = snapshot().joinToString("\n")
}

/** The trace the sign-in screen and the account request share. */
object SignInSteps : SignInTrace()

/**
 * The address of the connection that is heartbeating, not the saved public route.
 * Empty when this phone is not on a server connection.
 */
fun liveSignInServer(signalReached: String, serverUp: Boolean): String {
    if (!serverUp) return ""
    return signalReached.trim()
}

/** Drops signing keys and passwords from a diagnostics line. Ordinary sentences stay. */
fun redactDiagnostics(line: String): String {
    var out = line
    for (name in listOf("sign_pkcs8", "support_password", "supportPassword", "password", "auth_key")) {
        out = redactAssigned(out, name)
        out = redactJson(out, name)
    }
    return out
}

private fun redactAssigned(line: String, name: String): String {
    val key = "$name="
    val at = line.indexOf(key, ignoreCase = true)
    if (at < 0) return line
    val boundary = at == 0 || !line[at - 1].isLetterOrDigit()
    if (!boundary) return line
    val valueAt = at + key.length
    val end = line.indexOfAny(charArrayOf(' ', '\t'), valueAt).let { if (it < 0) line.length else it }
    return line.substring(0, valueAt) + "[redacted]" + line.substring(end)
}

private fun redactJson(line: String, name: String): String {
    val pattern = "\"$name\""
    val at = line.indexOf(pattern)
    if (at < 0) return line
    val colon = line.indexOf(':', at + pattern.length)
    if (colon < 0) return line
    var i = colon + 1
    while (i < line.length && line[i].isWhitespace()) i++
    if (i >= line.length || line[i] != '"') return line
    val start = i
    i++
    while (i < line.length && line[i] != '"') i++
    if (i >= line.length) return line
    return line.substring(0, start) + "\"[redacted]\"" + line.substring(i + 1)
}

/**
 * Addresses the sign-in screen dials itself. Empty: the engine resolves the
 * primary and standby, with a same-subnet LAN address ahead of the public one.
 */
fun signInRequestAddresses(storedPublic: List<String>): List<String> {
    if (storedPublic.any { it.isBlank() }) return emptyList()
    return emptyList()
}

/** Sign-in lines from a log file, newest last, with secrets removed. */
fun signInLinesFromLog(text: String, limit: Int = 50): String {
    val lines = text.lineSequence().map { redactDiagnostics(it) }.filter { it.contains("sign-in ") }.toList()
    return lines.takeLast(limit).joinToString("\n")
}

/** The Sign in button's first action. Returns the line it recorded. */
fun signInScreenPressed(trace: SignInTrace, heartbeat: String, op: String): String {
    val line = signInLogLine(heartbeat, "screen", "op=$op")
    trace.note(line)
    return line
}

/** Names the server that answered, unless the sentence already does. */
fun namedServerSignInFailure(server: String, message: String): String {
    val detail = message.trim().ifBlank { "did not accept the sign-in" }
    val who = server.trim()
    if (who.isEmpty() || detail.contains(" answered")) return detail
    return "The server at $who answered. $detail"
}

/**
 * The sentence on the Sign in screen. A raw "Sign in required" is the server
 * refusing the request before it checked the password.
 */
fun signInPageError(message: String, pauseSecs: Long = 0): String {
    val text = message.trim()
    val lower = text.lowercase()
    if (pauseSecs > 0 || lower.contains("too many sign-in attempts") || lower.contains("too many failed sign-ins")) {
        val secs = when {
            pauseSecs > 0 -> pauseSecs
            else -> signInPauseSeconds(429, text, null) ?: SIGN_IN_PAUSE_SECS
        }
        return signInPauseText(secs)
    }
    if (lower.contains("wrong email") || lower.contains("wrong password")) return "Wrong email or password."
    if (lower.contains("sign in required") || lower.contains("requires sign-in")) {
        return "This server is too old to accept a sign-in while Require signed-in accounts is on. Update the server."
    }
    if (lower.contains("did not answer the sign-in")) {
        return "The PJDesk server did not answer the sign-in."
    }
    if (lower.contains("step=")) return text
    if (text.isEmpty()) return "The sign-in did not finish."
    if (
        !lower.contains("answered") &&
        !lower.contains("is connected") &&
        (
            lower.contains("could not reach") ||
            lower.contains("timed out connecting") ||
            lower.contains("connection refused") ||
            lower.contains("network is unreachable")
        )
    ) {
        return "Could not reach the PJDesk server."
    }
    return text
}

/**
 * The sentence on the Sign in screen. A heartbeat means this phone already
 * reached that server, so a failed dial is not "could not reach".
 */
fun signInShownError(message: String, pauseSecs: Long = 0, heartbeat: String = ""): String {
    val page = signInPageError(message, pauseSecs)
    val where = heartbeat.trim()
    if (where.isNotEmpty() && page == "Could not reach the PJDesk server.") {
        return "The PJDesk server at $where is connected. The sign-in did not get an answer on that connection."
    }
    return page
}

/** The sentence on the Sign in screen. Minutes are the remaining time, rounded up. */
fun signInPauseText(remainingSecs: Long): String {
    val secs = remainingSecs.coerceAtLeast(0L)
    val minutes = ((secs + 59) / 60).coerceAtLeast(1)
    val unit = if (minutes == 1L) "minute" else "minutes"
    return "Too many sign-in attempts. Try again in $minutes $unit."
}

private fun waitMinutes(message: String): Long? {
    val match = Regex("""(\d+)\s*minute""").find(message.lowercase()) ?: return null
    return match.groupValues[1].toLongOrNull()
}
