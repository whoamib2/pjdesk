@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)

package us.spader.pjdesk.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
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

/** "3:07 PM" in the phone's time zone. */
fun chatClock(unix: Long): String =
    java.text.DateFormat.getTimeInstance(java.text.DateFormat.SHORT).format(java.util.Date(unix * 1000))

private val MineBubble = Color(0xFF0A84FF)
private val TheirBubble = Color(0xFF3A3A3C)
private val Header = Color(0xFF5E5CE6)
private val PeerName = Color(0xFFFF9F0A)
private val MeName = Color(0xFF64D2FF)

@Composable
private fun Bubble(line: ChatLine, showHeader: Boolean) {
    Column(
        Modifier.fillMaxWidth(),
        horizontalAlignment = if (line.mine) Alignment.End else Alignment.Start,
    ) {
        if (showHeader) {
            Row(Modifier.padding(top = 6.dp, bottom = 2.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(
                    if (line.mine) "You" else line.name,
                    color = if (line.mine) MeName else PeerName,
                    fontWeight = FontWeight.Bold,
                    fontSize = 12.sp,
                )
                Text(chatClock(line.unix), color = Color(0xFF8E8E93), fontSize = 12.sp)
            }
        }
        Surface(
            color = if (line.mine) MineBubble else TheirBubble,
            shape = if (line.mine) RoundedCornerShape(16.dp, 16.dp, 4.dp, 16.dp) else RoundedCornerShape(16.dp, 16.dp, 16.dp, 4.dp),
            modifier = Modifier.widthIn(max = 280.dp),
        ) {
            Text(
                line.text,
                color = Color.White,
                fontSize = 15.sp,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 7.dp),
            )
        }
    }
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
    val list = rememberLazyListState()
    val shown = lines.takeLast(200)
    LaunchedEffect(shown.size) {
        if (shown.isNotEmpty()) list.animateScrollToItem(shown.size - 1)
    }
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp),
        color = Color(0xFF1C1C1E),
        tonalElevation = 8.dp,
    ) {
        Column(Modifier.fillMaxWidth()) {
            Row(
                Modifier.fillMaxWidth().background(Header).padding(horizontal = 16.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(Modifier.size(10.dp).background(if (ready) Color(0xFF30D158) else Color(0xFF8E8E93), CircleShape))
                Text(
                    "Chat",
                    color = Color.White,
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.padding(start = 10.dp).weight(1f),
                )
                TextButton(onClick = onClose) { Text("Close", color = Color.White) }
            }
            if (tooOld) {
                Text(CHAT_NEEDS_TIP, color = Color(0xFFF2B03A), modifier = Modifier.padding(16.dp))
            } else {
                LazyColumn(
                    state = list,
                    modifier = Modifier.fillMaxWidth().heightIn(min = 140.dp, max = 320.dp).padding(horizontal = 12.dp),
                    verticalArrangement = Arrangement.spacedBy(3.dp),
                ) {
                    if (shown.isEmpty()) {
                        item {
                            Text(
                                "No messages yet. Say hello! This chat lasts for the session only.",
                                color = Color(0xFF8E8E93),
                                modifier = Modifier.padding(vertical = 16.dp),
                            )
                        }
                    }
                    itemsIndexed(shown) { i, line ->
                        val prev = shown.getOrNull(i - 1)
                        Bubble(line, prev == null || prev.mine != line.mine || prev.name != line.name)
                    }
                }
                Row(
                    Modifier.fillMaxWidth().background(Color(0xFF2C2C2E)).padding(8.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    OutlinedTextField(
                        value = draft,
                        onValueChange = { draft = it.take(2000) },
                        modifier = Modifier.weight(1f),
                        maxLines = 4,
                        enabled = ready,
                        placeholder = { Text("Message") },
                        shape = RoundedCornerShape(18.dp),
                    )
                    Button(
                        onClick = {
                            val text = draft.trim()
                            if (text.isNotEmpty()) {
                                onSend(text)
                                draft = ""
                            }
                        },
                        enabled = ready && draft.isNotBlank(),
                        shape = RoundedCornerShape(18.dp),
                        colors = androidx.compose.material3.ButtonDefaults.buttonColors(containerColor = MineBubble),
                    ) { Text("Send", color = Color.White) }
                }
            }
        }
    }
}
