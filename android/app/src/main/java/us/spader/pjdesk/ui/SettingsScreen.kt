package us.spader.pjdesk.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.core.DeskState

@Composable
fun SettingsScreen(
    state: DeskState,
    theme: String,
    onTheme: (String) -> Unit,
    onServer: (String) -> Unit,
    onResetFingerprint: () -> Unit,
    onSavePermanent: (String, String) -> Unit = { _, _ -> },
    onClearPermanent: () -> Unit = {},
    onUnattended: (Boolean) -> Unit = {},
    onShareLogs: () -> Unit = {},
) {
    var server by rememberSaveable(state.server) { mutableStateOf(state.server) }
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Settings", style = MaterialTheme.typography.titleLarge)
        Section("Server") {
            OutlinedTextField(
                value = server,
                onValueChange = { server = it },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                label = { Text("Address") },
                supportingText = { Text("Default is pjdesk.spader.us:21200") },
            )
            Button(onClick = { onServer(server.trim()) }) { Text("Save server") }
            Text("Key fingerprint", color = pjMuted(), style = MaterialTheme.typography.labelMedium)
            Text(
                state.fingerprint.ifBlank { "Not pinned yet. The first connection saves it." },
                fontFamily = FontFamily.Monospace,
                style = MaterialTheme.typography.bodyMedium,
            )
            TextButton(onClick = onResetFingerprint) { Text("Forget pinned key") }
        }
        Section("Security") {
            Text("Set permanent password", style = MaterialTheme.typography.titleMedium)
            PermanentPasswordForm(
                hasPermanent = state.hasPermanent,
                onSave = onSavePermanent,
                onClear = onClearPermanent,
            )
            WhenSomeoneConnects(
                hasPermanent = state.hasPermanent,
                unattended = state.approval == "unattended",
                onUnattended = onUnattended,
            )
        }
        Section("Display") {
            Text("Theme", color = pjMuted())
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf("system" to "System", "dark" to "Dark", "light" to "Light").forEach { (id, label) ->
                    FilterChip(selected = theme == id, onClick = { onTheme(id) }, label = { Text(label) })
                }
            }
        }
        Section("About") {
            Text("PJDesk 0.6.2", style = MaterialTheme.typography.titleMedium)
            Text(
                "Android host and controller. Video is H.264 Annex-B from the hardware encoder, the same stream the Windows client decodes with OpenH264. The server default is pjdesk.spader.us:21200.",
                color = pjMuted(),
            )
            TextButton(onClick = onShareLogs) { Text("Share logs") }
        }
    }
}

@Composable
private fun Section(title: String, content: @Composable () -> Unit) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            content()
        }
    }
}
