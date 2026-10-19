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
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.core.DeskState

@Composable
fun HomeScreen(
    state: DeskState,
    connectId: String,
    connectName: String = "",
    onConnectId: (String) -> Unit,
    onConnect: () -> Unit,
    onCopyId: () -> Unit,
    onCopyPassword: () -> Unit,
    onRegenerate: () -> Unit,
    onStop: () -> Unit,
    showPermanent: Boolean = false,
    onShowPermanent: (Boolean) -> Unit = {},
    onSavePermanent: (String, String) -> Boolean = { _, _ -> false },
    onClearPermanent: () -> Unit = {},
    onUnattended: (Boolean) -> Unit = {},
    onSignIn: (() -> Unit)? = null,
    accountEmail: String = "",
    onSignOut: (() -> Unit)? = null,
    temporary: Boolean = false,
) {
    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (accountEmail.isNotBlank()) {
            Card(modifier = Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (temporary) {
                        Text("Signed in until you close PJDesk")
                    } else {
                        Text("Signed in as $accountEmail")
                    }
                    if (onSignOut != null) {
                        Button(onClick = onSignOut, modifier = Modifier.fillMaxWidth()) { Text("Sign out") }
                    }
                }
            }
        }
        if (onSignIn != null) {
            Card(modifier = Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("Sign in with your PJDesk account for this network and its servers. That sign-in is not the password for a computer.")
                    Button(onClick = onSignIn) { Text("Sign in") }
                }
            }
        }
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
                OutlinedButton(
                    onClick = { onShowPermanent(!showPermanent) },
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text(if (showPermanent) "Hide permanent password" else "Set permanent password")
                }
                if (showPermanent) {
                    PermanentPasswordForm(
                        hasPermanent = state.hasPermanent,
                        onSave = onSavePermanent,
                        onClear = onClearPermanent,
                    )
                }
                WhenSomeoneConnects(
                    hasPermanent = state.hasPermanent,
                    unattended = state.approval == "unattended",
                    onUnattended = onUnattended,
                )
            }
        }
        Card(modifier = Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Connect", style = MaterialTheme.typography.titleMedium)
                OutlinedTextField(
                    value = connectId.filter { it.isDigit() }.take(9),
                    onValueChange = { onConnectId(it.filter { ch -> ch.isDigit() }.take(9)) },
                    modifier = Modifier.fillMaxWidth(),
                    singleLine = true,
                    label = { Text("Partner ID") },
                    visualTransformation = IdGroupTransformation,
                    textStyle = androidx.compose.ui.text.TextStyle(fontFamily = FontFamily.Monospace),
                )
                Text(
                    "A saved password is used on its own. Otherwise PJDesk asks after that computer answers.",
                    color = pjMuted(),
                    style = MaterialTheme.typography.labelMedium,
                )
                Button(onClick = onConnect, modifier = Modifier.fillMaxWidth()) { Text("Connect") }
            }
        }
    }
}

/** Groups of three digits. The field value stays raw digits, so the caret does not fight the spaces. */
private object IdGroupTransformation : VisualTransformation {
    override fun filter(text: AnnotatedString): TransformedText {
        val digits = text.text
        val out = StringBuilder()
        val map = ArrayList<Int>(digits.length + 1)
        for (i in digits.indices) {
            if (i > 0 && i % 3 == 0) out.append(' ')
            map.add(out.length)
            out.append(digits[i])
        }
        val transformed = out.toString()
        val offset = object : OffsetMapping {
            override fun originalToTransformed(offset: Int): Int {
                if (digits.isEmpty()) return 0
                val at = offset.coerceIn(0, digits.length)
                return if (at == digits.length) transformed.length else map[at]
            }
            override fun transformedToOriginal(offset: Int): Int {
                if (digits.isEmpty()) return 0
                val at = offset.coerceIn(0, transformed.length)
                var digitsBefore = 0
                var seen = 0
                for (ch in transformed) {
                    if (seen >= at) break
                    if (ch.isDigit()) digitsBefore++
                    seen++
                }
                return digitsBefore.coerceIn(0, digits.length)
            }
        }
        return TransformedText(AnnotatedString(transformed), offset)
    }
}

@Composable
fun ShellHint(text: String, onClick: () -> Unit) {
    OutlinedButton(onClick = onClick, modifier = Modifier.padding(16.dp)) { Text(text) }
}
