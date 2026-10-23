@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package us.spader.pjdesk.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import us.spader.pjdesk.AppLog
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.SavedFilter
import us.spader.pjdesk.core.SavedSort
import us.spader.pjdesk.core.arrangeSaved
import us.spader.pjdesk.core.computerTitle
import us.spader.pjdesk.core.formatSavedId
import us.spader.pjdesk.core.nickname
import us.spader.pjdesk.core.osDetail

@Composable
fun SavedScreen(
    computers: List<SavedComputer>,
    query: String,
    onQuery: (String) -> Unit,
    onForget: (SavedComputer) -> Unit,
    onRemovePassword: (SavedComputer) -> Unit = {},
    onRename: (SavedComputer, String) -> Unit = { _, _ -> },
    onConnect: (SavedComputer) -> Unit = {},
    recentIds: List<String> = emptyList(),
    selfId: String = "",
    signedIn: Boolean = false,
    status: String = "",
    sort: SavedSort = SavedSort.Online,
    filter: SavedFilter = SavedFilter.All,
    onSort: (SavedSort) -> Unit = {},
    onFilter: (SavedFilter) -> Unit = {},
) {
    var recentTab by remember { mutableStateOf(false) }
    val shown = arrangeSaved(computers, recentIds, sort, filter, query, recentOnly = recentTab)
    var editing by remember { mutableStateOf<SavedComputer?>(null) }
    var draft by remember { mutableStateOf("") }
    val self = selfId.filter { it.isDigit() }
    Column(
        Modifier.fillMaxSize().padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (status.isNotBlank()) {
            Card(Modifier.fillMaxWidth()) {
                Row(
                    Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    Text(status, style = MaterialTheme.typography.bodyMedium)
                }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = !recentTab, onClick = { recentTab = false }, label = { Text("Saved") })
            FilterChip(selected = recentTab, onClick = { recentTab = true }, label = { Text("Recent") })
        }
        Text(
            when {
                recentTab -> "Computers you have connected to on this phone."
                signedIn -> "This list syncs with your account. Saved passwords sync too, encrypted so the server cannot read them."
                else -> "Sign in at pjdesk.com to sync across your computers. This list stays on this phone."
            },
            color = pjMuted(),
            style = MaterialTheme.typography.bodyMedium,
        )
        OutlinedTextField(
            value = query,
            onValueChange = onQuery,
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            label = { Text("Search") },
            placeholder = { Text("Name, ID, or OS") },
        )
        if (!recentTab) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FilterChip(selected = sort == SavedSort.Online, onClick = { onSort(SavedSort.Online) }, label = { Text("Online first") })
                FilterChip(selected = sort == SavedSort.Name, onClick = { onSort(SavedSort.Name) }, label = { Text("Name") })
                FilterChip(selected = sort == SavedSort.Recent, onClick = { onSort(SavedSort.Recent) }, label = { Text("Recently used") })
            }
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = filter == SavedFilter.All, onClick = { onFilter(SavedFilter.All) }, label = { Text("All") })
            FilterChip(selected = filter == SavedFilter.Online, onClick = { onFilter(SavedFilter.Online) }, label = { Text("Online") })
            FilterChip(selected = filter == SavedFilter.Offline, onClick = { onFilter(SavedFilter.Offline) }, label = { Text("Offline") })
        }
        if (shown.isEmpty()) {
            Text(
                if (computers.isEmpty() && query.isBlank() && filter == SavedFilter.All) {
                    "Computers you connect to show up here."
                } else {
                    "No computers match."
                },
                color = pjMuted(),
                style = MaterialTheme.typography.bodyLarge,
            )
        }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(12.dp), modifier = Modifier.weight(1f)) {
            items(shown, key = { it.id }) { computer ->
                SavedComputerCard(
                    computer = computer,
                    mine = self.isNotBlank() && computer.id == self,
                    onConnect = {
                        AppLog.i("Saved row Connect ${computer.id} online=${computer.online}")
                        onConnect(computer)
                    },
                    onRename = {
                        editing = computer
                        draft = computer.nickname()
                    },
                    onRemovePassword = { onRemovePassword(computer) },
                    onForget = { onForget(computer) },
                )
            }
        }
    }
    val renaming = editing
    if (renaming != null) {
        Dialog(onDismissRequest = { editing = null }) {
            Card(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text("Rename", style = MaterialTheme.typography.titleLarge)
                    OutlinedTextField(
                        value = draft,
                        onValueChange = { draft = it.take(64) },
                        modifier = Modifier.fillMaxWidth(),
                        singleLine = true,
                        label = { Text("Nickname") },
                    )
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                        TextButton(onClick = { editing = null }) { Text("Cancel") }
                        TextButton(onClick = {
                            onRename(renaming, draft.trim())
                            editing = null
                        }) { Text("Save") }
                    }
                }
            }
        }
    }
}

@Composable
private fun SavedComputerCard(
    computer: SavedComputer,
    mine: Boolean,
    onConnect: () -> Unit,
    onRename: () -> Unit,
    onRemovePassword: () -> Unit,
    onForget: () -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    val title = computerTitle(computer)
    val host = computer.hostname.trim()
    val showHost = host.isNotBlank() && !host.equals(title, ignoreCase = true)
    val user = computer.consoleUser.trim()
    val idLine = formatSavedId(computer.id)
    val system = osDetail(computer)
    Card(Modifier.fillMaxWidth()) {
        Row(
            Modifier.padding(16.dp).fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            OsIcon(computer.osFamily.ifBlank { "unknown" }, Modifier.size(36.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    title,
                    style = MaterialTheme.typography.titleMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    color = if (title == "No name yet") pjMuted() else MaterialTheme.colorScheme.onSurface,
                )
                if (showHost) {
                    Text(host, color = pjMuted(), style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                if (user.isNotBlank()) {
                    Text(user, color = pjMuted(), style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                if (idLine.isNotBlank()) {
                    Text(idLine, color = pjMuted(), style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                if (system.isNotBlank()) {
                    Text(system, color = pjMuted(), style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                StatusChip(online = computer.online, mine = mine, known = computer.lastSeen > 0 || computer.online || computer.hostname.isNotBlank())
            }
            if (!mine) {
                if (computer.online) {
                    Button(onClick = onConnect) { Text("Connect") }
                } else {
                    OutlinedButton(onClick = onConnect) { Text("Connect") }
                }
            }
            Box {
                IconButton(onClick = { menu = true }) {
                    Icon(Icons.Filled.MoreVert, contentDescription = "More actions")
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(
                        text = { Text("Rename") },
                        onClick = {
                            menu = false
                            onRename()
                        },
                    )
                    if (computer.passwordSaved) {
                        DropdownMenuItem(
                            text = { Text("Remove password") },
                            onClick = {
                                menu = false
                                onRemovePassword()
                            },
                        )
                    }
                    DropdownMenuItem(
                        text = { Text("Remove") },
                        onClick = {
                            menu = false
                            onForget()
                        },
                    )
                }
            }
        }
    }
}

@Composable
private fun StatusChip(online: Boolean, mine: Boolean, known: Boolean) {
    val dark = MaterialTheme.colorScheme.background.luminance() < 0.5f
    val onlineColor = if (dark) OkDark else OkLight
    val idle = pjMuted()
    val (label, color) = when {
        mine -> "This phone" to MaterialTheme.colorScheme.primary
        online -> "Online" to onlineColor
        known -> "Offline" to idle
        else -> return
    }
    Surface(shape = RoundedCornerShape(50), color = color.copy(alpha = 0.16f)) {
        Row(
            Modifier.padding(horizontal = 8.dp, vertical = 3.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Box(Modifier.size(8.dp).clip(CircleShape).background(color))
            Text(label, color = color, style = MaterialTheme.typography.labelMedium, maxLines = 1)
        }
    }
}
