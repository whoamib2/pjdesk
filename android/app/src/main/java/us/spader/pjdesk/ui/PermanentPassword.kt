package us.spader.pjdesk.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
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
import us.spader.pjdesk.core.permanentHint
import us.spader.pjdesk.core.permanentIssue

/**
 * Status, Set or Change, and Remove. The password itself is typed in a dialog.
 * The one-time password on the home screen keeps working either way.
 */
@Composable
fun PermanentPasswordSection(
    hasPermanent: Boolean,
    onSave: (password: String, confirm: String) -> Boolean,
    onRemove: () -> Unit,
) {
    var open by rememberSaveable { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("Permanent password", style = MaterialTheme.typography.titleMedium)
        Text(
            if (hasPermanent) "A permanent password is set." else "No permanent password is set.",
            color = pjMuted(),
        )
        Text(
            "At least 8 characters, the same rule as Windows. It is not your PJDesk account password. Saved computers on your account can use it to connect.",
            color = pjMuted(),
            style = MaterialTheme.typography.bodySmall,
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(onClick = { open = true }) {
                Text(if (hasPermanent) "Change" else "Set")
            }
            if (hasPermanent) {
                TextButton(onClick = onRemove) { Text("Remove") }
            }
        }
    }
    if (open) {
        PermanentPasswordDialog(
            hasPermanent = hasPermanent,
            onDismiss = { open = false },
            onSave = { password, confirm ->
                if (onSave(password, confirm)) {
                    open = false
                    true
                } else {
                    false
                }
            },
            onRemove = {
                onRemove()
                open = false
            },
        )
    }
}

@Composable
internal fun PermanentPasswordFields(
    password: String,
    onPassword: (String) -> Unit,
    confirm: String,
    onConfirm: (String) -> Unit,
    issue: String?,
) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SecretField(value = password, onValueChange = onPassword, label = "Password")
        Text(permanentHint(password), color = pjMuted(), style = MaterialTheme.typography.bodySmall)
        SecretField(value = confirm, onValueChange = onConfirm, label = "Confirm password")
        if (issue != null) {
            Text(issue, color = MaterialTheme.colorScheme.error)
        }
    }
}

@Composable
private fun PermanentPasswordDialog(
    hasPermanent: Boolean,
    onDismiss: () -> Unit,
    onSave: (password: String, confirm: String) -> Boolean,
    onRemove: () -> Unit,
) {
    var password by rememberSaveable { mutableStateOf("") }
    var confirm by rememberSaveable { mutableStateOf("") }
    val issue = if (password.isEmpty() && confirm.isEmpty()) null else permanentIssue(password, confirm)
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (hasPermanent) "Change permanent password" else "Set permanent password") },
        text = {
            PermanentPasswordFields(
                password = password,
                onPassword = { password = it },
                confirm = confirm,
                onConfirm = { confirm = it },
                issue = issue,
            )
        },
        confirmButton = {
            TextButton(
                onClick = { onSave(password, confirm) },
                enabled = issue == null && password.isNotEmpty(),
            ) { Text("Save") }
        },
        dismissButton = {
            Row {
                if (hasPermanent) {
                    TextButton(onClick = onRemove) { Text("Remove") }
                }
                TextButton(onClick = onDismiss) { Text("Cancel") }
            }
        },
    )
}

/**
 * Ask me, or accept with the saved permanent password.
 * The second choice stays disabled until a password has been saved, then it
 * is a normal radio row (not a chip) so the tap target stays on screen.
 */
@Composable
fun WhenSomeoneConnects(
    hasPermanent: Boolean,
    unattended: Boolean,
    onUnattended: (Boolean) -> Unit,
) {
    val useSaved = hasPermanent && unattended
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text("When someone connects", style = MaterialTheme.typography.titleMedium)
        ChoiceRow(
            label = "Ask me",
            selected = !useSaved,
            enabled = true,
            onClick = { onUnattended(false) },
        )
        ChoiceRow(
            label = "Use the permanent password",
            selected = useSaved,
            enabled = hasPermanent,
            onClick = { onUnattended(true) },
        )
        if (!hasPermanent) {
            Text(
                "Save a permanent password before choosing this.",
                color = pjMuted(),
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}

/**
 * Off until the person turns it on. Turning it on is confirmed with a PIN or
 * fingerprint in the activity that owns this screen.
 */
@Composable
fun UnattendedAccessRow(on: Boolean, onChange: (Boolean) -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(
            Modifier.fillMaxWidth().clickable { onChange(!on) },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                "Unattended access (accept automatically)",
                modifier = Modifier.weight(1f),
                style = MaterialTheme.typography.titleMedium,
            )
            Switch(checked = on, onCheckedChange = onChange)
        }
        Text(
            "A connection that uses the permanent password, or a trusted account or computer, skips Allow and Deny. PJDesk then taps Android's screen-share dialog. Android 14 and later still ask once per session. The sharing notification stays on.",
            color = pjMuted(),
            style = MaterialTheme.typography.bodySmall,
        )
        Text(
            "This stays off until a permanent password is set, control is turned on, and you confirm with your PIN or fingerprint.",
            color = pjMuted(),
            style = MaterialTheme.typography.bodySmall,
        )
    }
}

/**
 * Accounts and computers that may view this phone without the PJDesk prompt.
 * Android's own screen-share consent still appears when capture is not already running.
 */
@Composable
fun TrustedViewAccess(
    accounts: List<String>,
    computers: List<String>,
    onAllowAccount: (String) -> Unit,
    onAllowComputer: (String) -> Unit,
    onForget: (String, String) -> Unit,
) {
    var account by rememberSaveable { mutableStateOf("") }
    var computer by rememberSaveable { mutableStateOf("") }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("Trusted view access", style = MaterialTheme.typography.titleMedium)
        Text(
            "Always allow a pjdesk.com account or a computer to view this phone without the Allow prompt. A permanent password with Use the permanent password does the same for every request. Android may still ask to share the screen the first time, and again after sharing stops.",
            color = pjMuted(),
        )
        OutlinedTextField(
            value = account,
            onValueChange = { account = it.take(80) },
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            label = { Text("Account, such as ben@spader.us") },
        )
        Button(
            onClick = {
                onAllowAccount(account)
                account = ""
            },
            enabled = account.contains("@"),
        ) { Text("Always allow this account") }
        accounts.forEach { saved ->
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(saved, modifier = Modifier.weight(1f))
                TextButton(onClick = { onForget("account", saved) }) { Text("Remove") }
            }
        }
        OutlinedTextField(
            value = computer,
            onValueChange = { computer = it.take(64) },
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            label = { Text("Computer name or ID") },
        )
        Button(
            onClick = {
                onAllowComputer(computer)
                computer = ""
            },
            enabled = computer.isNotBlank(),
        ) { Text("Always allow this computer") }
        computers.forEach { saved ->
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(saved, modifier = Modifier.weight(1f))
                TextButton(onClick = { onForget("computer", saved) }) { Text("Remove") }
            }
        }
    }
}

@Composable
private fun ChoiceRow(label: String, selected: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val shape = RoundedCornerShape(8.dp)
    val border = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .background(
                if (selected) MaterialTheme.colorScheme.primary.copy(alpha = 0.18f) else MaterialTheme.colorScheme.surface,
                shape,
            )
            .border(1.dp, if (enabled) border else border.copy(alpha = 0.45f), shape)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(end = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(selected = selected, onClick = if (enabled) onClick else null, enabled = enabled)
        Text(label, color = if (enabled) MaterialTheme.colorScheme.onSurface else pjMuted())
    }
}
