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
import us.spader.pjdesk.core.permanentOk

@Composable
fun SettingsScreen(
    state: DeskState,
    theme: String,
    onTheme: (String) -> Unit,
    onServer: (String) -> Unit,
    onResetFingerprint: () -> Unit,
    onPermanent: (String) -> Unit,
    onApproval: (String) -> Unit,
) {
    var server by rememberSaveable(state.server) { mutableStateOf(state.server) }
    var permanent by rememberSaveable { mutableStateOf("") }
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
            Text(
                "One-time password is on the home screen. A permanent password lets you leave this phone unattended. It has to be at least 8 characters.",
                color = pjMuted(),
            )
            SecretField(value = permanent, onValueChange = { permanent = it }, label = "Permanent password")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = { onPermanent(permanent) }, enabled = permanentOk(permanent)) { Text("Save") }
                TextButton(onClick = { permanent = ""; onPermanent("") }) { Text("Clear") }
            }
            if (permanent.isNotEmpty() && !permanentOk(permanent)) {
                Text("Use at least 8 characters.", color = MaterialTheme.colorScheme.error)
            }
            Text(if (state.hasPermanent) "A permanent password is set." else "No permanent password.", color = pjMuted())
            Text("When someone connects", style = MaterialTheme.typography.titleMedium)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FilterChip(
                    selected = state.approval != "unattended",
                    onClick = { onApproval("prompt") },
                    label = { Text("Ask me") },
                )
                FilterChip(
                    selected = state.approval == "unattended",
                    onClick = { onApproval("unattended") },
                    enabled = state.hasPermanent,
                    label = { Text("Use the permanent password") },
                )
            }
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
            Text("PJDesk 0.6.0", style = MaterialTheme.typography.titleMedium)
            Text(
                "Android host and controller. Video is H.264 Annex-B from the hardware encoder, the same stream the Windows client decodes with OpenH264. The server default is pjdesk.spader.us:21200.",
                color = pjMuted(),
            )
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
