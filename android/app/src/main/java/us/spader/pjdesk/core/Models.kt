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
    val view: Boolean = false,
    /** Trusted account, trusted computer, or the permanent password. Skip the PJDesk prompt. */
    val bypass: Boolean = false,
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
    /** Permission bits from Welcome. Negative until the host answers. */
    val permissions: Int = -1,
    val fps: Int = 0,
    val bitrateKbps: Int = 0,
    val latencyMs: Int = -1,
)

data class DeskState(
    val id: String = "",
    val idFormatted: String = "",
    val password: String = "",
    /** Temporary 8-digit code for Let someone connect. Empty until the engine has one. */
    val supportCode: String = "",
    /** Temporary password that matches [supportCode]. Not the permanent password. */
    val supportPassword: String = "",
    /** Unix seconds when the support pair was created. 0 until the first pair. */
    val supportBorn: Long = 0,
    val hasPermanent: Boolean = false,
    val fingerprint: String = "",
    val server: String = "",
    val approval: String = "prompt",
    /** Settings → Security. Off until a PIN or biometric turns it on. */
    val unattendedAccess: Boolean = false,
    val status: String = "Connecting…",
    /** This host session was accepted without a tap on the phone. */
    val acceptedAutomatically: Boolean = false,
    val incoming: IncomingCall? = null,
    val hosting: Boolean = false,
    val hostWho: String = "",
    val session: SessionInfo? = null,
    val error: String? = null,
    val notice: String? = null,
    val askPassword: Boolean = false,
    val askName: String = "",
    val askRejected: Boolean = false,
    /** Why the password dialog is open again, or empty on the first ask. */
    val askNote: String = "",
    /** LAN, Internet, or blank when the last signaling attempt failed. */
    val signalVia: String = "",
    val signalReached: String = "",
    val signalError: String = "",
    val serverLan: String = "",
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
    /** Last person signed in on that computer. Empty when the account did not share one. */
    val consoleUser: String = "",
    val lastSeen: Long = 0,
    val online: Boolean = false,
    /** Name chosen on that computer. Empty means show the hostname. */
    val displayName: String = "",
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

/** Length hint for the permanent-password dialog. The rule is still 8 characters. */
fun permanentHint(password: String): String {
    val count = password.trim().codePointCount(0, password.trim().length)
    return when {
        count == 0 -> "At least 8 characters. This is not your PJDesk account password."
        count < 8 -> "Use at least 8 characters."
        count < 12 -> "Long enough. A longer password is harder to guess."
        else -> "That is a strong length."
    }
}

/** What Settings and the home banner say about remote control. */
fun controlStatus(enabled: Boolean, bound: Boolean): String = when {
    enabled && bound -> "On. A computer can tap, type, and use the buttons."
    enabled -> "Listed in Accessibility, but not running. Turn PJDesk off and on again."
    else -> "Off. Open Accessibility and turn PJDesk on."
}

/** Empty when the password can be saved. Same 8-character rule as Windows. */
fun permanentIssue(password: String, confirm: String): String? {
    val pw = password.trim()
    val again = confirm.trim()
    if (pw.isEmpty()) return "Enter a password."
    if (!permanentOk(pw)) return "Use at least 8 characters."
    if (pw != again) return "Those passwords do not match."
    return null
}
