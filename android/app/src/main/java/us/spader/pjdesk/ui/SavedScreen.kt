@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package us.spader.pjdesk.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
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
import us.spader.pjdesk.core.nickname
import us.spader.pjdesk.core.savedSubtitle

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
    sort: SavedSort = SavedSort.Recent,
    filter: SavedFilter = SavedFilter.All,
    onSort: (SavedSort) -> Unit = {},
    onFilter: (SavedFilter) -> Unit = {},
) {
    var recentTab by remember { mutableStateOf(false) }
    var searchOpen by remember { mutableStateOf(query.isNotBlank()) }
    var menu by remember { mutableStateOf(false) }
    val shown = arrangeSaved(computers, recentIds, sort, filter, query, recentOnly = recentTab, selfId = selfId)
    var editing by remember { mutableStateOf<SavedComputer?>(null) }
    var draft by remember { mutableStateOf("") }
    val self = selfId.filter { it.isDigit() }
    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier
                .fillMaxWidth()
                .height(48.dp)
                .padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Segment("Saved", !recentTab) { recentTab = false }
            Segment("Recent", recentTab) { recentTab = true }
            Box(Modifier.weight(1f))
            if (!searchOpen) {
                IconButton(onClick = { searchOpen = true }, modifier = Modifier.size(48.dp)) {
                    SearchMark()
                }
            }
            Box {
                IconButton(onClick = { menu = true }, modifier = Modifier.size(48.dp)) {
                    FilterMark()
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    Text(
                        "Sort",
                        modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
                        style = MaterialTheme.typography.labelMedium,
                        color = pjMuted(),
                    )
                    listOf(
                        SavedSort.Recent to "Recently used",
                        SavedSort.Online to "Online first",
                        SavedSort.Name to "Name",
                    ).forEach { (value, label) ->
                        DropdownMenuItem(
                            text = { Text(if (sort == value) "$label  ✓" else label) },
                            onClick = { menu = false; onSort(value) },
                        )
                    }
                    HorizontalDivider()
                    Text(
                        "Show",
                        modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
                        style = MaterialTheme.typography.labelMedium,
                        color = pjMuted(),
                    )
                    listOf(
                        SavedFilter.All to "All",
                        SavedFilter.Online to "Online",
                        SavedFilter.Offline to "Offline",
                    ).forEach { (value, label) ->
                        DropdownMenuItem(
                            text = { Text(if (filter == value) "$label  ✓" else label) },
                            onClick = { menu = false; onFilter(value) },
                        )
                    }
                }
            }
        }
        if (searchOpen) {
            OutlinedTextField(
                value = query,
                onValueChange = onQuery,
                modifier = Modifier
                    .fillMaxWidth()
                    .height(52.dp)
                    .padding(horizontal = 12.dp),
                singleLine = true,
                textStyle = MaterialTheme.typography.bodyMedium,
                placeholder = { Text("Name, ID, or OS") },
                trailingIcon = {
                    TextButton(onClick = {
                        onQuery("")
                        searchOpen = false
                    }) { Text("Close") }
                },
            )
        }
        if (status.isNotBlank()) {
            Row(
                Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                Text(status, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        if (shown.isEmpty()) {
            Text(
                if (computers.isEmpty() && query.isBlank() && filter == SavedFilter.All) {
                    "Computers you connect to show up here."
                } else {
                    "No computers match."
                },
                color = pjMuted(),
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                style = MaterialTheme.typography.bodyMedium,
            )
        }
        LazyColumn(modifier = Modifier.weight(1f)) {
            items(shown, key = { it.id }) { computer ->
                SavedComputerRow(
                    computer = computer,
                    mine = self.isNotBlank() && computer.id.filter { it.isDigit() } == self,
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
private fun Segment(label: String, selected: Boolean, onClick: () -> Unit) {
    Surface(
        onClick = onClick,
        shape = RoundedCornerShape(8.dp),
        color = if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent,
    ) {
        Text(
            label,
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
            style = MaterialTheme.typography.labelLarge,
        )
    }
}

@Composable
private fun SavedComputerRow(
    computer: SavedComputer,
    mine: Boolean,
    onConnect: () -> Unit,
    onRename: () -> Unit,
    onRemovePassword: () -> Unit,
    onForget: () -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    val title = computerTitle(computer)
    val dark = MaterialTheme.colorScheme.background.luminance() < 0.5f
    val dot = when {
        mine -> MaterialTheme.colorScheme.primary
        computer.online -> if (dark) OkDark else OkLight
        else -> pjMuted()
    }
    val user = computer.consoleUser.trim()
    val host = computer.hostname.trim()
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp, max = 64.dp)
            .clickable(enabled = !mine, onClick = onConnect)
            .padding(horizontal = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(40.dp), contentAlignment = Alignment.Center) {
            OsIcon(computer.osFamily.ifBlank { "unknown" }, Modifier.size(28.dp))
            Box(
                Modifier
                    .align(Alignment.BottomEnd)
                    .size(10.dp)
                    .clip(CircleShape)
                    .background(dot),
            )
        }
        Column(Modifier.weight(1f).padding(horizontal = 8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(
                    title,
                    style = MaterialTheme.typography.bodyLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                    color = if (title == "No name yet") pjMuted() else MaterialTheme.colorScheme.onSurface,
                )
                if (mine) {
                    Text(
                        "This phone",
                        color = MaterialTheme.colorScheme.primary,
                        style = MaterialTheme.typography.labelSmall,
                        maxLines = 1,
                    )
                }
            }
            Text(
                savedSubtitle(computer),
                color = pjMuted(),
                style = MaterialTheme.typography.bodySmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (!mine) {
            IconButton(onClick = onConnect, modifier = Modifier.size(48.dp)) {
                ConnectMark(if (computer.online) MaterialTheme.colorScheme.primary else pjMuted())
            }
        }
        Box {
            IconButton(onClick = { menu = true }, modifier = Modifier.size(48.dp)) {
                Icon(Icons.Filled.MoreVert, contentDescription = "More actions")
            }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                if (user.isNotBlank()) {
                    DropdownMenuItem(text = { Text("User: $user") }, onClick = { menu = false }, enabled = false)
                }
                if (host.isNotBlank() && !host.equals(title, ignoreCase = true)) {
                    DropdownMenuItem(text = { Text(host) }, onClick = { menu = false }, enabled = false)
                }
                DropdownMenuItem(text = { Text("Rename") }, onClick = { menu = false; onRename() })
                if (computer.passwordSaved) {
                    DropdownMenuItem(text = { Text("Remove password") }, onClick = { menu = false; onRemovePassword() })
                }
                DropdownMenuItem(text = { Text("Remove") }, onClick = { menu = false; onForget() })
            }
        }
    }
}

@Composable
private fun SearchMark() {
    val color = MaterialTheme.colorScheme.onSurface
    androidx.compose.foundation.Canvas(Modifier.size(22.dp)) {
        drawCircle(color, radius = size.minDimension * 0.28f, center = androidx.compose.ui.geometry.Offset(size.width * 0.42f, size.height * 0.42f), style = androidx.compose.ui.graphics.drawscope.Stroke(width = 1.8.dp.toPx()))
        drawLine(color, androidx.compose.ui.geometry.Offset(size.width * 0.62f, size.height * 0.62f), androidx.compose.ui.geometry.Offset(size.width * 0.84f, size.height * 0.84f), strokeWidth = 1.8.dp.toPx())
    }
}

@Composable
private fun FilterMark() {
    val color = MaterialTheme.colorScheme.onSurface
    androidx.compose.foundation.Canvas(Modifier.size(22.dp)) {
        drawLine(color, androidx.compose.ui.geometry.Offset(3.dp.toPx(), 5.dp.toPx()), androidx.compose.ui.geometry.Offset(size.width - 3.dp.toPx(), 5.dp.toPx()), strokeWidth = 1.7.dp.toPx())
        drawLine(color, androidx.compose.ui.geometry.Offset(6.dp.toPx(), size.height / 2), androidx.compose.ui.geometry.Offset(size.width - 6.dp.toPx(), size.height / 2), strokeWidth = 1.7.dp.toPx())
        drawLine(color, androidx.compose.ui.geometry.Offset(9.dp.toPx(), size.height - 5.dp.toPx()), androidx.compose.ui.geometry.Offset(size.width - 9.dp.toPx(), size.height - 5.dp.toPx()), strokeWidth = 1.7.dp.toPx())
    }
}

@Composable
private fun ConnectMark(color: Color) {
    androidx.compose.foundation.Canvas(Modifier.size(22.dp)) {
        val path = androidx.compose.ui.graphics.Path().apply {
            moveTo(size.width * 0.28f, size.height * 0.18f)
            lineTo(size.width * 0.82f, size.height * 0.5f)
            lineTo(size.width * 0.28f, size.height * 0.82f)
            close()
        }
        drawPath(path, color)
    }
}
