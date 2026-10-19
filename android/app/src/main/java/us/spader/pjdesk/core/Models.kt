package us.spader.pjdesk.core

data class DisplayInfo(
    val index: Int,
    val name: String,
    val width: Int,
    val height: Int,
    val primary: Boolean,
)

data class IncomingCall(
    val name: String,
    val ip: String,
)

data class SessionInfo(
    val peer: String,
    val id: String,
    val path: String,
    val platform: String,
    val version: String,
    val layout: Boolean,
    val displays: List<DisplayInfo>,
    val frameW: Int = 0,
    val frameH: Int = 0,
)

data class DeskState(
    val id: String = "",
    val idFormatted: String = "",
    val password: String = "",
    val hasPermanent: Boolean = false,
    val fingerprint: String = "",
    val server: String = "",
    val approval: String = "prompt",
    val status: String = "Connecting…",
    val incoming: IncomingCall? = null,
    val hosting: Boolean = false,
    val session: SessionInfo? = null,
    val error: String? = null,
    val notice: String? = null,
)

data class SavedComputer(
    val id: String,
    /** Optional nickname. Empty when the user has not named this computer. */
    val name: String = "",
    val osFamily: String = "",
    val osVersion: String = "",
    val passwordSaved: Boolean = false,
    /** Hostname the remote computer reported. Empty before the first time it was seen. */
    val hostname: String = "",
)

fun compactId(text: String): String = text.filter { !it.isWhitespace() }

/** True when `text` is the same 9-digit ID, ignoring spaces. */
fun isIdLabel(text: String, id: String): Boolean {
    val a = compactId(text)
    return a.isNotEmpty() && a.equals(compactId(id), ignoreCase = true)
}

/** A name the user typed. The ID and the computer's own hostname are not nicknames. */
fun SavedComputer.nickname(): String {
    val alias = name.trim()
    if (alias.isEmpty() || isIdLabel(alias, id)) return ""
    if (hostname.isNotBlank() && alias.equals(hostname.trim(), ignoreCase = true)) return ""
    return alias
}

fun permanentOk(password: String): Boolean =
    password.trim().codePointCount(0, password.trim().length) >= 8

/** Empty when the password can be saved. Same 8-character rule as Windows. */
fun permanentIssue(password: String, confirm: String): String? {
    val pw = password.trim()
    val again = confirm.trim()
    if (pw.isEmpty()) return "Enter a password."
    if (!permanentOk(pw)) return "Use at least 8 characters."
    if (pw != again) return "Those passwords do not match."
    return null
}
