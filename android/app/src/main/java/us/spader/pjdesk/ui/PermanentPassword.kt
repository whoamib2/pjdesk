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
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
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
import us.spader.pjdesk.core.permanentIssue

/**
 * Permanent password, with a confirm field and Show/Hide on both.
 * The one-time password on the home screen keeps working either way.
 */
@Composable
fun PermanentPasswordForm(
    hasPermanent: Boolean,
    onSave: (password: String, confirm: String) -> Unit,
    onClear: () -> Unit,
) {
    var password by rememberSaveable { mutableStateOf("") }
    var confirm by rememberSaveable { mutableStateOf("") }
    val issue = if (password.isEmpty() && confirm.isEmpty()) null else permanentIssue(password, confirm)
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            "A permanent password lets you leave this phone unattended. It has to be at least 8 characters, the same rule as Windows.",
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
                    onSave(password, confirm)
                    password = ""
                    confirm = ""
                },
                enabled = issue == null && password.isNotEmpty(),
            ) { Text("Save") }
            TextButton(onClick = {
                password = ""
                confirm = ""
                onClear()
            }) { Text("Clear") }
        }
        Text(if (hasPermanent) "A permanent password is set." else "No permanent password.", color = pjMuted())
    }
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
