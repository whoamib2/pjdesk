package us.spader.pjdesk.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.AppLog
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.nickname

@Composable
fun SavedScreen(
    computers: List<SavedComputer>,
    query: String,
    onQuery: (String) -> Unit,
    onOpen: (SavedComputer) -> Unit,
    onForget: (SavedComputer) -> Unit,
    onRemovePassword: (SavedComputer) -> Unit = {},
    onRename: (SavedComputer, String) -> Unit = { _, _ -> },
    onConnect: (SavedComputer) -> Unit = {},
    recentIds: List<String> = emptyList(),
    selfId: String = "",
    signedIn: Boolean = false,
    status: String = "",
) {
    var recentTab by remember { mutableStateOf(false) }
    val source = if (recentTab) {
        recentIds.map { id -> computers.find { it.id == id } ?: SavedComputer(id = id) }
    } else {
        computers
    }
    val q = query.trim()
    val shown = source.filter { matches(it, q) }
    var editing by remember { mutableStateOf<String?>(null) }
    var draft by remember { mutableStateOf("") }
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Saved computers", style = MaterialTheme.typography.titleLarge)
        if (status.isNotBlank()) {
            Card(Modifier.fillMaxWidth()) {
                Text(status, Modifier.padding(12.dp), style = MaterialTheme.typography.bodyMedium)
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            TextButton(onClick = { recentTab = false }) {
                Text("Saved", color = if (recentTab) pjMuted() else MaterialTheme.colorScheme.primary)
            }
            TextButton(onClick = { recentTab = true }) {
                Text("Recent", color = if (recentTab) MaterialTheme.colorScheme.primary else pjMuted())
            }
        }
        Text(
            when {
                recentTab -> "Computers you have connected to on this phone."
                signedIn -> "This list syncs with your account. Passwords stay on this phone."
                else -> "Sign in at pjdesk.com to sync across your computers. This list stays on this phone."
            },
            color = pjMuted(),
            style = MaterialTheme.typography.bodySmall,
        )
        OutlinedTextField(
            value = query,
            onValueChange = onQuery,
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            label = { Text("Nickname, name, ID, or OS") },
        )
        if (shown.isEmpty()) {
            Text(
                if (computers.isEmpty()) "Computers you connect to show up here." else "No computers match that search.",
                color = pjMuted(),
            )
        }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(shown, key = { it.id }) { computer ->
                val nick = computer.nickname()
                val host = computer.hostname.trim()
                val display = computer.displayName.trim().let { if (it.equals(host, ignoreCase = true)) "" else it }
                val title = nick.ifBlank { display.ifBlank { host.ifBlank { "No name yet" } } }
                val system = osLabel(computer.osFamily).ifBlank { "Unknown" }
                val mine = selfId.isNotBlank() && computer.id == selfId.filter { it.isDigit() }
                Card(Modifier.fillMaxWidth()) {
                    Row(
                        Modifier.padding(12.dp).fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        OsIcon(computer.osFamily.ifBlank { "unknown" })
                        Column(
                            Modifier
                                .weight(1f)
                                .clickable(enabled = !mine && editing != computer.id) { onOpen(computer) },
                        ) {
                            Text(
                                title,
                                style = MaterialTheme.typography.titleMedium,
                                color = if (nick.isBlank() && host.isBlank()) pjMuted() else MaterialTheme.colorScheme.onSurface,
                            )
                            if (nick.isNotBlank() && host.isNotBlank()) {
                                Text(host, color = pjMuted(), style = MaterialTheme.typography.bodySmall)
                            }
                            if (computer.consoleUser.isNotBlank()) {
                                Text(computer.consoleUser, color = pjMuted(), style = MaterialTheme.typography.bodySmall)
                            }
                            val status = when {
                                mine -> "This phone"
                                computer.online -> "Online"
                                computer.lastSeen > 0 -> "Offline"
                                else -> ""
                            }
                            if (status.isNotEmpty()) {
                                Text(status, color = pjMuted(), style = MaterialTheme.typography.bodySmall)
                            }
                            val detail = buildString {
                                append(computer.id.chunked(3).joinToString(" "))
                                append(" · ").append(system)
                                if (computer.osVersion.isNotEmpty()) append(" ").append(computer.osVersion)
                            }
                            Text(detail, color = pjMuted(), fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.labelMedium)
                            if (editing == computer.id) {
                                OutlinedTextField(
                                    value = draft,
                                    onValueChange = { draft = it },
                                    modifier = Modifier.fillMaxWidth(),
                                    singleLine = true,
                                    label = { Text("Nickname") },
                                )
                                Row {
                                    TextButton(onClick = {
                                        onRename(computer, draft)
                                        editing = null
                                    }) { Text("Save") }
                                    TextButton(onClick = { editing = null }) { Text("Cancel") }
                                }
                            }
                        }
                        Column {
                            if (!mine) {
                                Button(onClick = {
                                    AppLog.i("Saved row Connect ${computer.id} online=${computer.online}")
                                    onConnect(computer)
                                }) { Text("Connect") }
                            }
                            TextButton(onClick = {
                                editing = computer.id
                                draft = nick
                            }) { Text("Rename") }
                            if (computer.passwordSaved) {
                                TextButton(onClick = { onRemovePassword(computer) }) { Text("Remove password") }
                            }
                            TextButton(onClick = { onForget(computer) }) { Text("Remove") }
                        }
                    }
                }
            }
        }
    }
}

private fun matches(computer: SavedComputer, query: String): Boolean {
    if (query.isBlank()) return true
    val q = query.lowercase()
    val qid = q.filter { !it.isWhitespace() }
    val id = computer.id.filter { !it.isWhitespace() }.lowercase()
    val system = osLabel(computer.osFamily)
    return computer.nickname().lowercase().contains(q)
        || computer.hostname.lowercase().contains(q)
        || computer.displayName.lowercase().contains(q)
        || (qid.isNotEmpty() && id.contains(qid))
        || computer.osFamily.lowercase().contains(q)
        || computer.consoleUser.lowercase().contains(q)
        || system.lowercase().contains(q)
}
