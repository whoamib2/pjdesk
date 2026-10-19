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
    val server: String = "pjdesk.spader.us:21200",
    val approval: String = "prompt",
    val status: String = "Connecting…",
    val incoming: IncomingCall? = null,
    val hosting: Boolean = false,
    val session: SessionInfo? = null,
    val error: String? = null,
)

data class SavedComputer(
    val id: String,
    val name: String,
    val osFamily: String = "",
    val osVersion: String = "",
)

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
