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
            if (!ready) {
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
