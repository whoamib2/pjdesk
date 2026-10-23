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
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.core.AppLockPolicy

@Composable
fun SetPinScreen(
    email: String,
    onEmail: (String) -> Unit,
    needPassword: Boolean,
    busy: Boolean,
    error: String,
    biometricsAvailable: Boolean,
    biometricsDefault: Boolean = biometricsAvailable,
    title: String,
    detail: String,
    onSave: (pin: String, password: String, enableBio: Boolean) -> Unit,
    onCancel: (() -> Unit)?,
) {
    var pin by rememberSaveable { mutableStateOf("") }
    var confirm by rememberSaveable { mutableStateOf("") }
    var password by rememberSaveable { mutableStateOf("") }
    var enableBio by rememberSaveable { mutableStateOf(biometricsDefault) }
    var localError by rememberSaveable { mutableStateOf("") }
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(title, style = MaterialTheme.typography.headlineMedium)
        Text(detail, color = pjMuted())
        if (needPassword) {
            AccountEmailField(value = email, onValueChange = onEmail, label = "Email")
            AccountPasswordField(value = password, onValueChange = { password = it }, label = "Account password")
            Text("This is your pjdesk.com password, used once to unlock saved computer passwords.", color = pjMuted())
        }
        SecretField(value = pin, onValueChange = { pin = it.filter { ch -> ch.isDigit() }.take(8) }, label = "App PIN")
        SecretField(value = confirm, onValueChange = { confirm = it.filter { ch -> ch.isDigit() }.take(8) }, label = "Confirm PIN")
        Text("4 to 8 digits. PJDesk asks for this each time you open the app.", color = pjMuted())
        if (biometricsAvailable) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Checkbox(checked = enableBio, onCheckedChange = { enableBio = it }, enabled = !busy)
                Text("Unlock with fingerprint or face")
            }
        }
        val shown = localError.ifBlank { error }
        if (shown.isNotEmpty()) Text(shown, color = MaterialTheme.colorScheme.error)
        Button(
            onClick = {
                localError = when {
                    needPassword && password.isBlank() -> "Enter your account password."
                    !AppLockPolicy.pinAcceptable(pin) -> "Use 4 to 8 digits."
                    pin != confirm -> "Those PINs do not match."
                    else -> ""
                }
                if (localError.isEmpty()) onSave(pin, password, enableBio && biometricsAvailable)
            },
            enabled = !busy,
            modifier = Modifier.fillMaxWidth(),
        ) { Text(if (busy) "Saving…" else "Save PIN") }
        if (onCancel != null) TextButton(onClick = onCancel, enabled = !busy) { Text("Cancel") }
    }
}

@Composable
fun UnlockScreen(
    busy: Boolean,
    error: String,
    biometrics: Boolean,
    onSubmit: (String) -> Unit,
    onBiometric: () -> Unit,
) {
    var pin by rememberSaveable { mutableStateOf("") }
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Unlock PJDesk", style = MaterialTheme.typography.headlineMedium)
        Text("Your account stays signed in. This only opens the app.", color = pjMuted())
        SecretField(value = pin, onValueChange = { pin = it.filter { ch -> ch.isDigit() }.take(8) }, label = "App PIN")
        if (error.isNotEmpty()) Text(error, color = MaterialTheme.colorScheme.error)
        Button(
            onClick = { if (AppLockPolicy.pinAcceptable(pin)) onSubmit(pin) },
            enabled = !busy,
            modifier = Modifier.fillMaxWidth(),
        ) { Text(if (busy) "Checking…" else "Unlock") }
        if (biometrics) TextButton(onClick = onBiometric, enabled = !busy) { Text("Use fingerprint or face") }
    }
}

@Composable
fun ReauthScreen(
    email: String,
    onEmail: (String) -> Unit,
    busy: Boolean,
    error: String,
    onSubmit: (String) -> Unit,
) {
    var password by rememberSaveable { mutableStateOf("") }
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Sign in with your password", style = MaterialTheme.typography.headlineMedium)
        Text(
            "The PIN was not accepted too many times, or saved passwords need your account password again. You will set a new PIN after this.",
            color = pjMuted(),
        )
        AccountEmailField(value = email, onValueChange = onEmail, label = "Email")
        AccountPasswordField(value = password, onValueChange = { password = it }, label = "Account password")
        if (error.isNotEmpty()) Text(error, color = MaterialTheme.colorScheme.error)
        Button(
            onClick = { if (password.isNotBlank()) onSubmit(password) },
            enabled = !busy,
            modifier = Modifier.fillMaxWidth(),
        ) { Text(if (busy) "Signing in…" else "Sign in with your password") }
    }
}
