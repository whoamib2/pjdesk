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
    if (
        text.isEmpty() ||
        lower.contains("did not answer") ||
        lower.contains("could not reach") ||
        lower.contains("timed out") ||
        lower.contains("closed the connection")
    ) {
        return "Could not reach the PJDesk server."
    }
    return text
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
