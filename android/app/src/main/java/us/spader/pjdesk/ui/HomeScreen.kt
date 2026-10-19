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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.core.DeskState

@Composable
fun HomeScreen(
    state: DeskState,
    connectId: String,
    connectPassword: String,
    onConnectId: (String) -> Unit,
    onConnectPassword: (String) -> Unit,
    onConnect: () -> Unit,
    onCopyId: () -> Unit,
    onCopyPassword: () -> Unit,
    onRegenerate: () -> Unit,
    onStop: () -> Unit,
    showPermanent: Boolean = false,
    onShowPermanent: (Boolean) -> Unit = {},
    onSavePermanent: (String, String, Boolean) -> Unit = { _, _, _ -> },
    onClearPermanent: () -> Unit = {},
    onUnattended: (Boolean) -> Unit = {},
) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("This phone", style = MaterialTheme.typography.titleLarge)
        Text(state.status, color = pjMuted(), style = MaterialTheme.typography.bodyMedium)
        if (state.hosting) {
            Card(modifier = Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("Being controlled", style = MaterialTheme.typography.titleMedium)
                    Text("Someone is viewing and controlling this phone.", color = pjMuted())
                    Button(onClick = onStop) { Text("Stop") }
                }
            }
        }
        Card(modifier = Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Your ID", color = pjMuted(), style = MaterialTheme.typography.labelMedium)
                Text(
                    state.idFormatted.ifBlank { "Waiting for the server" },
                    style = MaterialTheme.typography.headlineMedium,
                    fontFamily = FontFamily.Monospace,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    TextButton(onClick = onCopyId, enabled = state.id.isNotEmpty()) { Text("Copy ID") }
                }
                SecretField(
                    value = state.password,
                    onValueChange = {},
                    label = "One-time password",
                    readOnly = true,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    TextButton(onClick = onCopyPassword, enabled = state.password.isNotEmpty()) { Text("Copy") }
                    TextButton(onClick = onRegenerate) { Text("New password") }
                }
                if (state.hasPermanent) {
                    Text("A permanent password is set.", color = pjMuted(), style = MaterialTheme.typography.bodyMedium)
                }
                TextButton(onClick = { onShowPermanent(!showPermanent) }) {
                    Text(if (showPermanent) "Hide permanent password" else "Set permanent password")
                }
                if (showPermanent) {
                    PermanentPasswordForm(
                        hasPermanent = state.hasPermanent,
                        unattended = state.approval == "unattended",
                        onSave = onSavePermanent,
                        onClear = onClearPermanent,
                        onUnattended = onUnattended,
                    )
                }
            }
        }
        Card(modifier = Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Connect", style = MaterialTheme.typography.titleMedium)
                OutlinedTextField(
                    value = connectId,
                    onValueChange = onConnectId,
                    modifier = Modifier.fillMaxWidth(),
                    singleLine = true,
                    label = { Text("Partner ID") },
                )
                SecretField(
                    value = connectPassword,
                    onValueChange = onConnectPassword,
                    label = "Password",
                )
                Button(onClick = onConnect, modifier = Modifier.fillMaxWidth()) { Text("Connect") }
            }
        }
    }
}

@Composable
fun ShellHint(text: String, onClick: () -> Unit) {
    OutlinedButton(onClick = onClick, modifier = Modifier.padding(16.dp)) { Text(text) }
}
