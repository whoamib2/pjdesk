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

@Composable
fun SignInScreen(
    email: String,
    password: String,
    code: String,
    useCode: Boolean,
    busy: Boolean,
    error: String,
    onEmail: (String) -> Unit,
    onPassword: (String) -> Unit,
    onCode: (String) -> Unit,
    onUseCode: (Boolean) -> Unit,
    onSubmit: () -> Unit,
) {
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Sign in", style = MaterialTheme.typography.headlineMedium)
        Text(
            "Use your PJDesk account. This phone gets your private server from the account.",
            color = pjMuted(),
        )
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (!useCode) {
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
                    Text("From pjdesk.com → My network. One device, one hour.", color = pjMuted())
                }
                if (error.isNotEmpty()) Text(error, color = MaterialTheme.colorScheme.error)
                Button(onClick = onSubmit, enabled = !busy, modifier = Modifier.fillMaxWidth()) {
                    Text(if (busy) "Signing in…" else "Sign in")
                }
                TextButton(onClick = { onUseCode(!useCode) }, enabled = !busy) {
                    Text(if (useCode) "Use email and password" else "I have an enrollment code")
                }
            }
        }
    }
}
