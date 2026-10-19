package us.spader.pjdesk.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.core.permanentIssue

/**
 * Permanent password, with a confirm field and Show/Hide on both.
 * "Accept with password without asking" is the Windows unattended mode.
 */
@Composable
fun PermanentPasswordForm(
    hasPermanent: Boolean,
    unattended: Boolean,
    onSave: (password: String, confirm: String, unattended: Boolean) -> Unit,
    onClear: () -> Unit,
    onUnattended: (Boolean) -> Unit,
) {
    var password by rememberSaveable { mutableStateOf("") }
    var confirm by rememberSaveable { mutableStateOf("") }
    var allowUnattended by rememberSaveable(unattended) { mutableStateOf(unattended) }
    val issue = if (password.isEmpty() && confirm.isEmpty()) null else permanentIssue(password, confirm)
    val canUnattended = hasPermanent || (password.isNotEmpty() && issue == null)
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            "A permanent password lets you leave this phone unattended. It has to be at least 8 characters, the same rule as Windows. The one-time password on the home screen still works.",
            color = pjMuted(),
        )
        SecretField(value = password, onValueChange = { password = it }, label = "Permanent password")
        SecretField(value = confirm, onValueChange = { confirm = it }, label = "Confirm password")
        if (issue != null) {
            Text(issue, color = MaterialTheme.colorScheme.error)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(
                onClick = {
                    onSave(password, confirm, allowUnattended)
                    password = ""
                    confirm = ""
                },
                enabled = issue == null && password.isNotEmpty(),
            ) { Text("Save") }
            TextButton(onClick = {
                password = ""
                confirm = ""
                allowUnattended = false
                onClear()
            }) { Text("Clear") }
        }
        Text(if (hasPermanent) "A permanent password is set." else "No permanent password.", color = pjMuted())
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Switch(
                checked = allowUnattended && canUnattended,
                onCheckedChange = { on ->
                    allowUnattended = on
                    if (hasPermanent) onUnattended(on)
                },
                enabled = canUnattended,
            )
            Text("Accept with password without asking")
        }
        if (!canUnattended) {
            Text("Set a permanent password before turning this on.", color = pjMuted(), style = MaterialTheme.typography.bodySmall)
        }
    }
}
