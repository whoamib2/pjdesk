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
import androidx.compose.material3.Switch
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
import us.spader.pjdesk.core.controlStatus

@Composable
fun SettingsScreen(
    state: DeskState,
    theme: String,
    onTheme: (String) -> Unit,
    onServer: (String) -> Unit,
    onResetFingerprint: () -> Unit,
    onSavePermanent: (String, String) -> Boolean = { _, _ -> false },
    onClearPermanent: () -> Unit = {},
    controlEnabled: Boolean = false,
    controlBound: Boolean = false,
    onOpenControl: () -> Unit = {},
    onUnattended: (Boolean) -> Unit = {},
    onUnattendedAccess: (Boolean) -> Unit = {},
    trustedAccounts: List<String> = emptyList(),
    trustedComputers: List<String> = emptyList(),
    onAllowAccount: (String) -> Unit = {},
    onAllowComputer: (String) -> Unit = {},
    onForgetTrusted: (String, String) -> Unit = { _, _ -> },
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
    signedIn: Boolean = false,
    hasPin: Boolean = false,
    biometricsOn: Boolean = false,
    biometricsAvailable: Boolean = false,
    autoLockSeconds: Int = 120,
    onChangePin: () -> Unit = {},
    onToggleBiometrics: (Boolean) -> Unit = {},
    onAutoLock: (Int) -> Unit = {},
    engineDetail: String = "",
    accountEmail: String = "",
    onSignOut: (() -> Unit)? = null,
    remote: us.spader.pjdesk.core.RemoteOptions = us.spader.pjdesk.core.RemoteOptions(),
    onRemote: (us.spader.pjdesk.core.RemoteOptions) -> Unit = {},
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
        Section("Diagnostics") {
            DiagnosticLine("Engine", engineDetail.ifBlank { "Starting PJDesk…" })
            DiagnosticLine("Server", state.server.ifBlank { "Not set" })
            DiagnosticLine("LAN", state.serverLan.ifBlank { "None published" })
            val path = when {
                state.signalError.isNotBlank() -> state.signalError
                state.signalVia.isBlank() -> "Not connected yet"
                else -> "${state.signalVia} ${state.signalReached}".trim()
            }
            DiagnosticLine("Path", path)
            DiagnosticLine(
                "Relay",
                if (state.signalReached.isNotBlank()) state.signalReached else state.server.ifBlank { "Not set" },
            )
            DiagnosticLine("Account", accountEmail.ifBlank { "Not signed in" })
        }
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
        Section("Control") {
            Text(controlStatus(controlEnabled, controlBound))
            Button(onClick = onOpenControl, modifier = Modifier.fillMaxWidth()) {
                Text(if (controlEnabled && controlBound) "Open Accessibility" else us.spader.pjdesk.service.ControlNotice.TEXT)
            }
        }
        Section("Security") {
            PermanentPasswordSection(
                hasPermanent = state.hasPermanent,
                onSave = onSavePermanent,
                onRemove = onClearPermanent,
            )
            WhenSomeoneConnects(
                hasPermanent = state.hasPermanent,
                unattended = state.approval == "unattended",
                onUnattended = onUnattended,
            )
            UnattendedAccessRow(on = state.unattendedAccess, onChange = onUnattendedAccess)
            TrustedViewAccess(
                accounts = trustedAccounts,
                computers = trustedComputers,
                onAllowAccount = onAllowAccount,
                onAllowComputer = onAllowComputer,
                onForget = onForgetTrusted,
            )
        }
        Section("Remote control") {
            Text(
                "These are the defaults for every computer. A session can remember its own. The same idle time applies in the foreground, in the background, and when the screen is off. In the foreground, PJDesk warns 10 seconds before it disconnects.",
                color = pjMuted(),
            )
            RemoteOptionsForm(options = remote, onChange = onRemote)
        }
        Section("Display") {
            Text("Theme", color = pjMuted())
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf("system" to "System", "dark" to "Dark", "light" to "Light").forEach { (id, label) ->
                    FilterChip(selected = theme == id, onClick = { onTheme(id) }, label = { Text(label) })
                }
            }
        }
        if (signedIn) {
            Section("App lock") {
                Text("PJDesk asks for the PIN or biometrics when you open it, and after it has been in the background.", color = pjMuted())
                if (hasPin) {
                    Button(onClick = onChangePin, modifier = Modifier.fillMaxWidth()) { Text("Change PIN") }
                }
                if (biometricsAvailable) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                        Text("Unlock with fingerprint or face", modifier = Modifier.weight(1f))
                        Switch(checked = biometricsOn, onCheckedChange = onToggleBiometrics)
                    }
                }
                Text("Lock after", color = pjMuted())
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf(60 to "1 min", 120 to "2 min", 300 to "5 min").forEach { (seconds, label) ->
                        FilterChip(
                            selected = autoLockSeconds == seconds,
                            onClick = { onAutoLock(seconds) },
                            label = { Text(label) },
                        )
                    }
                }
            }
        }
        Section("Sign-in") {
            Text(
                "Saved computers sync with your account. Saved passwords sync too, encrypted so the server cannot read them.",
                color = pjMuted(),
            )
            if (onSignOut != null) {
                Button(onClick = onSignOut, modifier = Modifier.fillMaxWidth()) { Text("Sign out") }
            }
            Text("Temporary sign-in", style = MaterialTheme.typography.titleMedium)
            Text(
                "Minutes with no PJDesk activity before a temporary sign-in ends. The sign-in checkbox starts unchecked, so this phone stays signed in.",
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
            Text("PJDesk 0.6.23", style = MaterialTheme.typography.titleMedium)
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
private fun DiagnosticLine(label: String, value: String) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(label, color = pjMuted(), style = MaterialTheme.typography.labelMedium)
        Text(value, style = MaterialTheme.typography.bodyLarge)
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
