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
import androidx.compose.material3.OutlinedButton
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
    onSavePermanent: (String, String) -> Boolean = { _, _ -> false },
    onClearPermanent: () -> Unit = {},
    onUnattended: (Boolean) -> Unit = {},
    onShareLogs: () -> Unit = {},
    onImportConfig: () -> Unit = {},
    onExportConfig: () -> Unit = {},
    importOffer: String? = null,
    importPass: String = "",
    onImportPass: (String) -> Unit = {},
    onAcceptImport: () -> Unit = {},
    onDismissImport: () -> Unit = {},
    computerName: String = "",
    deviceLabel: String = "",
    onComputerName: (String) -> Unit = {},
    tempIdleMinutes: Int = 30,
    onTempIdleMinutes: (Int) -> Unit = {},
    onCheckUpdate: () -> Unit = {},
) {
    var server by rememberSaveable(state.server) { mutableStateOf(state.server) }
    var name by rememberSaveable(computerName) { mutableStateOf(computerName) }
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Settings", style = MaterialTheme.typography.titleLarge)
        Section("This phone") {
            OutlinedTextField(
                value = name,
                onValueChange = { name = it.take(64) },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                label = { Text("This phone's name") },
                supportingText = {
                    val shown = deviceLabel.ifBlank { "this phone" }
                    Text("Computer name: $shown. Leave blank to use that. Other computers see this name unless they set a nickname.")
                },
            )
            Button(onClick = { onComputerName(name.trim()) }) { Text("Save name") }
        }
        Section("ID/Relay Server") {
            if (importOffer != null) {
                ServerConfigOffer(
                    blob = importOffer,
                    pass = importPass,
                    onPass = onImportPass,
                    onImport = onAcceptImport,
                    onCancel = onDismissImport,
                    fromPaste = false,
                )
            }
            OutlinedButton(onClick = onImportConfig, modifier = Modifier.fillMaxWidth()) {
                Text("Import server config")
            }
            OutlinedButton(onClick = onExportConfig, modifier = Modifier.fillMaxWidth()) {
                Text("Export server config")
            }
            Text(
                "Import reads a network key from the clipboard and applies it. Export copies this phone's network key. The text is encrypted, so it does not show the server address.",
                color = pjMuted(),
            )
            OutlinedTextField(
                value = server,
                onValueChange = { server = it },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                label = { Text("Address") },
                supportingText = { Text("Filled in when you sign in. Add :port if it is not 21200.") },
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
        Section("Sign-in") {
            Text("Temporary sign-in", style = MaterialTheme.typography.titleMedium)
            Text(
                "Minutes with no PJDesk activity before a temporary sign-in ends. The sign-in checkbox on the sign-in screen starts checked.",
                color = pjMuted(),
            )
            var idle by rememberSaveable(tempIdleMinutes) { mutableStateOf(tempIdleMinutes.toString()) }
            OutlinedTextField(
                value = idle,
                onValueChange = {
                    idle = it.filter { ch -> ch.isDigit() }.take(4)
                    idle.toIntOrNull()?.let { n -> onTempIdleMinutes(n.coerceIn(1, 24 * 60)) }
                },
                singleLine = true,
                label = { Text("Idle minutes") },
            )
        }
        Section("About") {
            Text("PJDesk 0.6.7", style = MaterialTheme.typography.titleMedium)
            OutlinedButton(onClick = onCheckUpdate, modifier = Modifier.fillMaxWidth()) {
                Text("Check for updates")
            }
            Text(
                "Android host and controller. Video is H.264 Annex-B from the hardware encoder, the same stream the Windows client decodes with OpenH264. The server comes from your PJDesk account after you sign in.",
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
