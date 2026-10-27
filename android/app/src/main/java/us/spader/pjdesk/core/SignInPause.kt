package us.spader.pjdesk.core

/** The account login window when pjdesk.com does not say how long to wait. */
const val SIGN_IN_PAUSE_SECS = 15 * 60L

/**
 * Seconds to wait after HTTP 429 from `/api/login`. Null for any other failure.
 * The production body is "Too many failed sign-ins. Wait 15 minutes and try again."
 * A code-guess or PIN lock ("Too many tries") is not this pause.
 */
fun signInPauseSeconds(status: Int, message: String): Long? {
    val limited = (status == 429 && !message.contains("Too many tries"))
        || message.contains("Too many failed sign-ins")
        || message.contains("Too many sign-in attempts")
    if (!limited) return null
    return (waitMinutes(message) ?: (SIGN_IN_PAUSE_SECS / 60)) * 60
}

/** Ben's sentence, with the remaining minutes rounded up and a ticking clock. */
fun signInPauseText(remainingSecs: Long): String {
    val secs = remainingSecs.coerceAtLeast(0L)
    val minutes = ((secs + 59) / 60).coerceAtLeast(1)
    val unit = if (minutes == 1L) "minute" else "minutes"
    val mm = (secs / 60).toInt()
    val ss = (secs % 60).toInt()
    val clock = "%02d:%02d".format(mm, ss)
    return "Too many sign-in attempts from this computer, so pjdesk.com paused sign-ins for a bit. Your password may be fine. Wait $minutes $unit, then try once (each try restarts the wait). $clock"
}

private fun waitMinutes(message: String): Long? {
    val match = Regex("""(\d+)\s*minute""").find(message.lowercase()) ?: return null
    return match.groupValues[1].toLongOrNull()
}
