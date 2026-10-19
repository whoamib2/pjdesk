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
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.core.SavedComputer

@Composable
fun SavedScreen(
    computers: List<SavedComputer>,
    query: String,
    onQuery: (String) -> Unit,
    onOpen: (SavedComputer) -> Unit,
    onForget: (SavedComputer) -> Unit,
    onRemovePassword: (SavedComputer) -> Unit = {},
) {
    val q = query.trim().lowercase()
    val shown = computers.filter {
        q.isEmpty() || it.name.lowercase().contains(q) || it.id.contains(q) || it.osFamily.lowercase().contains(q)
    }
    Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Saved computers", style = MaterialTheme.typography.titleLarge)
        OutlinedTextField(
            value = query,
            onValueChange = onQuery,
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            label = { Text("Search") },
        )
        if (shown.isEmpty()) {
            Text(
                if (computers.isEmpty()) "Computers you connect to show up here." else "No computers match that search.",
                color = pjMuted(),
            )
        }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(shown, key = { it.id }) { computer ->
                Card(Modifier.fillMaxWidth().clickable { onOpen(computer) }) {
                    Row(
                        Modifier.padding(12.dp).fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        OsIcon(computer.osFamily.ifBlank { "unknown" })
                        Column(Modifier.weight(1f)) {
                            Text(computer.name.ifBlank { computer.id }, style = MaterialTheme.typography.titleMedium)
                            val detail = buildString {
                                append(computer.id.chunked(3).joinToString(" "))
                                val label = osLabel(computer.osFamily)
                                if (label.isNotEmpty()) append(" · ").append(label)
                                if (computer.osVersion.isNotEmpty()) append(" ").append(computer.osVersion)
                            }
                            Text(detail, color = pjMuted(), fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.labelMedium)
                        }
                        Column {
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
