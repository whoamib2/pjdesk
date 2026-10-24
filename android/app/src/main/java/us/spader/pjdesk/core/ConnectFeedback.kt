package us.spader.pjdesk.core

/**
 * Sentence for a failed Connect, or null when this attempt was replaced or the password was dismissed.
 * [name] is the computer the person tapped. The password itself is never part of the text.
 */
fun connectFailureText(raw: String, name: String): String? {
    val text = raw.trim()
    val head = text.lineSequence().firstOrNull()?.trim().orEmpty()
    if (head.isEmpty() || head == "stopped" || head == "cancelled") return null
    val who = name.trim().ifBlank { "That computer" }
    val lower = head.lowercase()
    return when {
        lower.contains("even through the relay") -> "Could not reach $who, even through the relay."
        lower.contains("could not reach the pjdesk server") ||
            lower.contains("timed out connecting to the server") ||
            lower.contains("cannot reach the pjdesk server") ->
            "Could not reach the PJDesk server, so $who could not be connected."
        lower.contains("did not respond") ||
            lower.contains("did not finish setting up") ||
            lower.contains("is offline") ->
            if (head.contains(who)) head else "$who is offline, or it did not answer."
        lower.contains("server address is empty") || lower.contains("no server address") ->
            "This phone has no server address yet. Sign in again, or set the server in Settings."
        lower.contains("locked out") -> head
        lower == "no answer" || lower.startsWith("no answer") -> "No answer. $who did not accept."
        lower == "declined" || lower.startsWith("declined") -> "Declined. $who did not allow the connection."
        lower.contains("no one is available") -> "No one is available to accept."
        else -> head
    }
}

/** Status line shown the moment Connect starts. */
fun connectStatusLabel(name: String, id: String): String {
    val who = name.trim().ifBlank {
        id.filter { it.isDigit() }.chunked(3).joinToString(" ").ifBlank { "that computer" }
    }
    return "Connecting to $who…"
}
