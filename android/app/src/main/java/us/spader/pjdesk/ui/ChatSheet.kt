@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)

package us.spader.pjdesk.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.core.ChatLine

const val CHAT_NEEDS_TIP = "The other computer needs PJDesk 0.6.50 or newer"
const val CHAT_OFF_TIP = "Chat is not allowed in this session by the other computer"
const val CHAT_WAIT_TIP = "Chat is not available in this session yet"

/** Why chat is off, matching Windows: the version message only for a known old version. */
fun chatOffTip(gate: ChatGate): String = when (gate) {
    ChatGate.TooOld -> CHAT_NEEDS_TIP
    ChatGate.Ready -> CHAT_OFF_TIP
    ChatGate.Unknown -> CHAT_WAIT_TIP
}

enum class ChatGate { Ready, TooOld, Unknown }

/** A PJDesk release is 0.x. Windows 10 and 11 are not. A pre-release is returned with the dash. */
fun pjdeskAppVersion(raw: String): String? {
    val trimmed = raw.trim().removePrefix("v")
    val core = trimmed.substringBefore('-').substringBefore('+')
    val parts = core.split('.')
    if (parts.size < 2 || parts.any { it.toIntOrNull() == null }) return null
    if (parts[0].toIntOrNull() != 0) return null
    return trimmed
}

fun chatVersionAtLeast(version: String): Boolean {
    if (version.contains('-')) return false
    val parts = version.substringBefore('+').split('.')
    val have = intArrayOf(
        parts.getOrNull(0)?.toIntOrNull() ?: return false,
        parts.getOrNull(1)?.toIntOrNull() ?: 0,
        parts.getOrNull(2)?.toIntOrNull() ?: 0,
    )
    val need = intArrayOf(0, 6, 50)
    val index = (0..2).firstOrNull { have[it] != need[it] } ?: return true
    return have[index] > need[index]
}

/**
 * Handshake version wins. Fresh presence is the fallback. A saved version that
 * is not fresh does not count. The old-version warning is [ChatGate.TooOld] only.
 */
fun chatGate(handshake: String, presence: String, presenceFresh: Boolean): ChatGate {
    val version = pjdeskAppVersion(handshake) ?: if (presenceFresh) pjdeskAppVersion(presence) else null
    if (version == null) return ChatGate.Unknown
    return if (chatVersionAtLeast(version)) ChatGate.Ready else ChatGate.TooOld
}

fun chatButtonOn(chatReady: Boolean, gate: ChatGate): Boolean = gate != ChatGate.TooOld && (chatReady || gate == ChatGate.Ready)

fun showChatNeedsTip(chatReady: Boolean, gate: ChatGate): Boolean = gate == ChatGate.TooOld && !chatReady

fun formatChatLine(line: ChatLine): String {
    val secs = (line.unix % 86_400).toInt().coerceAtLeast(0)
    val h = secs / 3600
    val m = (secs % 3600) / 60
    val s = secs % 60
    return "%02d:%02d:%02d UTC  %s  %s".format(h, m, s, line.name, line.text)
}

@Composable
fun ChatSheet(
    lines: List<ChatLine>,
    ready: Boolean,
    tooOld: Boolean = false,
    onSend: (String) -> Unit,
    onClose: () -> Unit,
) {
    var draft by remember { mutableStateOf("") }
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp),
        color = MaterialTheme.colorScheme.surface,
        tonalElevation = 8.dp,
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Chat", style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                TextButton(onClick = onClose) { Text("Close") }
            }
            if (tooOld) {
                Text(CHAT_NEEDS_TIP, color = MaterialTheme.colorScheme.tertiary)
            } else {
                Column(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(min = 120.dp, max = 280.dp)
                        .verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    if (lines.isEmpty()) {
                        Text("No messages yet. This chat lasts for the session only.", color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    lines.takeLast(40).forEach { line ->
                        Text(formatChatLine(line), style = MaterialTheme.typography.bodyMedium)
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(
                        value = draft,
                        onValueChange = { draft = it.take(2000) },
                        modifier = Modifier.weight(1f),
                        singleLine = true,
                        label = { Text("Message") },
                    )
                    Button(
                        onClick = {
                            val text = draft.trim()
                            if (text.isNotEmpty()) {
                                onSend(text)
                                draft = ""
                            }
                        },
                        enabled = draft.isNotBlank(),
                    ) { Text("Send") }
                }
            }
        }
    }
}
