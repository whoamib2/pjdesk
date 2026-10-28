package us.spader.pjdesk.core

/**
 * Sentence for a failed Connect, or null when this attempt was replaced or the password was dismissed.
 * [name] is the computer the person tapped. The password itself is never part of the text.
 */
fun connectFailureText(raw: String, name: String, listedOnline: Boolean = false): String? {
    val text = raw.trim()
    val head = text.lineSequence().firstOrNull()?.trim().orEmpty()
    if (head.isEmpty() || head == "stopped" || head == "cancelled") return null
    val who = name.trim().ifBlank { "That computer" }
    val lower = head.lowercase()
    return when {
        lower.contains("your sign-in expired") || lower.contains("sign in required") -> "Your sign-in expired"
        lower.contains("even through the relay") -> "Could not reach $who, even through the relay."
        lower.contains("no udp reply") -> head
        lower.contains("could not reach the pjdesk server at") -> publicServerFailure(head, who)
        lower.contains("not on the server's local network") || lower.contains("not on that server's local network") ->
            if (head.contains(who)) head else head.replace(Regex("so .+ could not be connected\\.?$"), "so $who could not be connected.")
        lower.contains("could not reach the pjdesk server") ||
            lower.contains("timed out connecting to the server") ||
            lower.contains("cannot reach the pjdesk server") ->
            "Could not reach the PJDesk server, so $who could not be connected."
        lower.contains("did not finish the relay") || lower.contains("did not finish setting up") ->
            "$who is on the PJDesk server, but it did not finish the relay setup."
        lower.contains("did not answer through the pjdesk server") ||
            lower.contains("did not start a session") ||
            lower.contains("did not respond") ||
            lower.contains("is offline, or it did not answer") ->
            if (listedOnline) {
                "$who did not answer through the PJDesk server. It is listed as online, so this phone could not start a session on the server it reached."
            } else {
                "$who did not answer through the PJDesk server. This phone reached the server, but that computer did not start a session."
            }
        lower.contains("is offline") -> if (head.contains(who)) head else "$who is offline."
        lower.contains("server address is empty") || lower.contains("no server address") ->
            "This phone has no server address yet. Sign in again, or set the server in Settings."
        lower.contains("locked out") -> head
        lower == "no answer" || lower.startsWith("no answer") -> "No answer. $who did not accept."
        lower == "declined" || lower.startsWith("declined") -> "Declined. $who did not allow the connection."
        lower.contains("no one is available") -> "No one is available to accept."
        else -> head
    }
}

private fun publicServerFailure(head: String, who: String): String {
    val addr = head.substringAfter("at ", "").substringBefore(" (").trim().ifBlank { "its public address" }
    val why = head.substringAfter("(", "").substringBefore(")").trim()
    val reason = if (why.isBlank()) "" else " ($why)"
    return "Could not reach the PJDesk server at $addr$reason. This phone is not on that server's local network, so $who could not be connected."
}

/** Home and Settings. The email is the account. The session is valid, expired, or not checked yet. */
fun accountStatusLine(email: String, session: String, temporary: Boolean): String {
    val who = email.trim()
    if (who.isBlank()) return "Not signed in"
    if (temporary) return "Signed in as $who until you close PJDesk. Session valid."
    return when (session) {
        "valid" -> "Signed in as $who. Session valid."
        "expired" -> "Signed in as $who. Session expired."
        else -> "Signed in as $who. Session not checked."
    }
}

/** A refresh that was refused. A server that is down is not an expired session. */
fun sessionAfterRefreshFailure(message: String): String? {
    val lower = message.lowercase()
    return if (
        lower.contains("sign in required") ||
        lower.contains("wrong email") ||
        lower.contains("expired") ||
        lower.contains("revoked")
    ) {
        "expired"
    } else {
        null
    }
}

/** Skip the local "offline" block when the list would still say the computer was here just now. */
fun blockOfflineConnect(online: Boolean, lastSeen: Long, nowMs: Long): Boolean {
    if (online) return false
    val seen = seenSeconds(lastSeen)
    if (seen <= 0L) return true
    val age = nowMs / 1000 - seen
    return age !in 0..59
}

/** Extra Connect attempts after "not online" when the computer was seen recently. */
const val NOT_ONLINE_RETRIES = 2

/** A last-seen time inside this window is recent enough to retry. */
const val NOT_ONLINE_RECENT_MS = 120_000L

/**
 * True when Connect should wait and try again.
 * [attempt] is how many extra tries have already been started. [lastSeen] is unix seconds or milliseconds.
 */
fun shouldRetryNotOnline(message: String, lastSeen: Long, nowMs: Long, attempt: Int): Boolean {
    if (attempt >= NOT_ONLINE_RETRIES) return false
    val head = message.lineSequence().firstOrNull()?.trim().orEmpty()
    if (!head.lowercase().contains("not online")) return false
    val seenMs = seenSeconds(lastSeen) * 1000
    if (seenMs <= 0L) return false
    val age = nowMs - seenMs
    return age in 0..NOT_ONLINE_RECENT_MS
}

/** Status line shown the moment Connect starts. */
/** One line safe to keep after logcat rotates. Tokens and keys are removed. */
fun redactConnectError(message: String): String {
    var text = message.replace(
        Regex("(?i)(password|token|auth_key|sign_pkcs8|refresh|secret|support_password)\\s*[:=]\\s*\\S+"),
        "$1=(hidden)",
    )
    text = text.replace(Regex("\\b[0-9a-fA-F]{32,}\\b"), "(hidden)")
    return text.replace('\n', ' ').replace('\r', ' ').take(400)
}

fun connectStatusLabel(name: String, id: String): String {
    val who = name.trim().ifBlank {
        id.filter { it.isDigit() }.chunked(3).joinToString(" ").ifBlank { "that computer" }
    }
    return "Connecting to $who…"
}
