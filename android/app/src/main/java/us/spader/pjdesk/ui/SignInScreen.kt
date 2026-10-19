package us.spader.pjdesk.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.core.NetKey

@Composable
fun SignInScreen(
    email: String,
    password: String,
    code: String,
    useCode: Boolean,
    useKey: Boolean = false,
    blob: String = "",
    keyPass: String = "",
    busy: Boolean,
    error: String,
    onEmail: (String) -> Unit,
    onPassword: (String) -> Unit,
    onCode: (String) -> Unit,
    onBlob: (String) -> Unit = {},
    onKeyPass: (String) -> Unit = {},
    onUseCode: (Boolean) -> Unit,
    onUseKey: (Boolean) -> Unit = {},
    onScan: () -> Unit = {},
    accountForKey: Boolean = false,
    onSubmit: () -> Unit,
    importOffer: String? = null,
    importPass: String = "",
    onImportPass: (String) -> Unit = {},
    onAcceptImport: () -> Unit = {},
    onDismissImport: () -> Unit = {},
    offerCurrent: Boolean = false,
    onUseCurrent: () -> Unit = {},
) {
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (importOffer != null) {
            ServerConfigOffer(
                blob = importOffer,
                pass = importPass,
                onPass = onImportPass,
                onImport = onAcceptImport,
                onCancel = onDismissImport,
                fromPaste = true,
            )
        }
        Text("Sign in", style = MaterialTheme.typography.headlineMedium)
        Text(
            if (useKey) "Paste a network key, or scan its QR code. This phone enrolls and stays signed in after a restart."
            else "Use your PJDesk account. This phone gets your private server from the account.",
            color = pjMuted(),
        )
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (useKey) {
                    OutlinedTextField(
                        value = blob,
                        onValueChange = onBlob,
                        modifier = Modifier.fillMaxWidth(),
                        minLines = 3,
                        label = { Text("Network key") },
                    )
                    if (NetKey.kind(blob) != "embedded") {
                        SecretField(value = keyPass, onValueChange = onKeyPass, label = "Passphrase")
                        Text("Only if the key was created with a passphrase.", color = pjMuted())
                    } else {
                        Text("Anyone who has this network key can join.", color = MaterialTheme.colorScheme.error)
                    }
                    OutlinedButton(onClick = onScan, enabled = !busy, modifier = Modifier.fillMaxWidth()) {
                        Text("Scan QR code")
                    }
                    if (accountForKey || error.contains("specific account")) {
                        OutlinedTextField(
                            value = email,
                            onValueChange = onEmail,
                            modifier = Modifier.fillMaxWidth(),
                            singleLine = true,
                            label = { Text("Account email") },
                        )
                        SecretField(value = password, onValueChange = onPassword, label = "Account password")
                    }
                } else if (!useCode) {
                    OutlinedTextField(
                        value = email,
                        onValueChange = onEmail,
                        modifier = Modifier.fillMaxWidth(),
                        singleLine = true,
                        label = { Text("Email") },
                    )
                    SecretField(value = password, onValueChange = onPassword, label = "Password")
                } else {
                    OutlinedTextField(
                        value = code,
                        onValueChange = onCode,
                        modifier = Modifier.fillMaxWidth(),
                        singleLine = true,
                        label = { Text("Enrollment code") },
                    )
                    Text("From pjdesk.com → Account → My network. One device, one hour.", color = pjMuted())
                }
                if (error.isNotEmpty()) Text(error, color = MaterialTheme.colorScheme.error)
                if (offerCurrent) {
                    Button(onClick = onUseCurrent, enabled = !busy, modifier = Modifier.fillMaxWidth()) {
                        Text("Use this computer's current server for my network")
                    }
                }
                Button(onClick = onSubmit, enabled = !busy, modifier = Modifier.fillMaxWidth()) {
                    Text(if (busy) "Signing in…" else if (useKey) "Use network key" else "Sign in")
                }
                if (useKey || useCode) {
                    TextButton(onClick = { onUseKey(false); onUseCode(false) }, enabled = !busy) {
                        Text("Use email and password")
                    }
                }
                if (!useCode) {
                    TextButton(onClick = { onUseKey(false); onUseCode(true) }, enabled = !busy) {
                        Text("I have an enrollment code")
                    }
                }
                if (!useKey) {
                    TextButton(onClick = { onUseCode(false); onUseKey(true) }, enabled = !busy) {
                        Text("Use a network key")
                    }
                }
            }
        }
    }
}

@Composable
fun ServerConfigOffer(
    blob: String,
    pass: String,
    onPass: (String) -> Unit,
    onImport: () -> Unit,
    onCancel: () -> Unit,
    fromPaste: Boolean,
) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                if (fromPaste) "Import this server config?" else "Import server config",
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                if (fromPaste) "A network key was pasted. Importing it sets this phone's server and enrolls it."
                else "Enter the passphrase for this server config. It was shared separately from the key.",
                color = pjMuted(),
            )
            when (NetKey.kind(blob)) {
                "embedded" -> Text("Anyone who has this network key can join.", color = MaterialTheme.colorScheme.error)
                "passphrase" -> SecretField(value = pass, onValueChange = onPass, label = "Passphrase")
            }
            Button(onClick = onImport, modifier = Modifier.fillMaxWidth()) { Text("Import server config") }
            TextButton(onClick = onCancel) { Text("Cancel") }
        }
    }
}
