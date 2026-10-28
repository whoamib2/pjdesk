package us.spader.pjdesk.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Checkbox
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
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
import us.spader.pjdesk.core.signInLogLine
import us.spader.pjdesk.core.signInScreenPressed
import us.spader.pjdesk.core.SignInTrace

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
    temporary: Boolean = false,
    onTemporary: (Boolean) -> Unit = {},
    onEnrollOnly: () -> Unit = {},
    signInPaused: Boolean = false,
    onClose: (() -> Unit)? = null,
    serverAddress: String = "",
    signInTrace: SignInTrace? = null,
    onSignInTrace: (String) -> Unit = {},
    onCopyDiagnostics: () -> Unit = {},
) {
    PjPage {
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
        if (onClose != null) {
            TextButton(
                onClick = onClose,
                colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.onBackground),
            ) { Text("Back") }
        }
        Text(
            "Sign in to your network",
            style = MaterialTheme.typography.headlineMedium,
            color = MaterialTheme.colorScheme.onBackground,
        )
        Text(
            if (useKey) "Paste a network key, or scan its QR code. This phone enrolls and stays signed in after a restart."
            else if (useCode) "Enter an 8-digit code from someone on this network. It works once, lasts one hour, and you do not need a PJDesk account. An older enrollment code still works in this box."
            else "Your PJDesk account sets the servers for this network. It does not connect you to a computer, and it is not that computer's password.",
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
                    AccountEmailField(value = email, onValueChange = onEmail, label = "Account email")
                    AccountPasswordField(value = password, onValueChange = onPassword, label = "Account password")
                    }
                } else if (!useCode) {
                    AccountEmailField(value = email, onValueChange = onEmail, label = "Email")
                    AccountPasswordField(value = password, onValueChange = onPassword, label = "PJDesk account password")
                } else {
                    OutlinedTextField(
                        value = code,
                        onValueChange = onCode,
                        modifier = Modifier.fillMaxWidth(),
                        singleLine = true,
                        label = { Text("8-digit code") },
                        placeholder = { Text("1234 5678") },
                    )
                    Text(
                        "From Invite on a signed-in PJDesk. One use, one hour. An older enrollment code from pjdesk.com still works here.",
                        color = pjMuted(),
                    )
                }
                Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                    Checkbox(checked = temporary, onCheckedChange = onTemporary, enabled = !busy)
                    Text("Sign me out when I'm done (use on someone else's computer)")
                }
                Text(
                    "This computer is added to your account either way. When this is checked, the sign-in ends when you close PJDesk and is not kept on this phone.",
                    color = pjMuted(),
                )
                if (error.isNotEmpty()) {
                    Text(error, color = MaterialTheme.colorScheme.error)
                    TextButton(onClick = onCopyDiagnostics) { Text("Copy diagnostics") }
                }
                if (offerCurrent) {
                    Button(onClick = onUseCurrent, enabled = !busy, modifier = Modifier.fillMaxWidth()) {
                        Text("Use this computer's current server for my network")
                    }
                }
                val emailSignIn = !useCode && !useKey
                val canSignIn = !busy && !(signInPaused && emailSignIn)
                val signInOp = if (useKey) "key" else if (useCode) "code" else "sign-in"
                Button(
                    onClick = {
                        // First action of this button, before the password derivation or the network.
                        val line = if (signInTrace != null) {
                            signInScreenPressed(signInTrace, serverAddress, signInOp)
                        } else {
                            signInLogLine(serverAddress, "screen", "op=$signInOp")
                        }
                        onSignInTrace(line)
                        onSubmit()
                    },
                    enabled = canSignIn,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(
                        if (busy) "Signing in…"
                        else if (useKey) "Use network key"
                        else if (useCode) "Join"
                        else "Sign in",
                    )
                }
                if (!useCode && !useKey) {
                    TextButton(
                        onClick = {
                            val line = if (signInTrace != null) {
                                signInScreenPressed(signInTrace, serverAddress, "enroll")
                            } else {
                                signInLogLine(serverAddress, "screen", "op=enroll")
                            }
                            onSignInTrace(line)
                            onEnrollOnly()
                        },
                        enabled = canSignIn,
                    ) {
                        Text("Just add this computer to my account")
                    }
                }
                if (useKey || useCode) {
                    TextButton(onClick = { onUseKey(false); onUseCode(false) }, enabled = !busy) {
                        Text("Use email and password")
                    }
                }
                if (!useCode) {
                    TextButton(onClick = { onUseKey(false); onUseCode(true) }, enabled = !busy) {
                        Text("Join with an 8-digit code")
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
