package us.spader.pjdesk.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.materialIcon
import androidx.compose.material.icons.materialPath
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties

/** The one line under the title. A refusal or the computer's own note wins. */
fun connectPasswordMessage(note: String, rejected: Boolean, hadSaved: Boolean): String = when {
    note.isNotBlank() -> note.trim().lineSequence().first()
    rejected && hadSaved -> "Saved password didn't work. Enter it again."
    rejected -> "That password didn't work. Try again."
    else -> "Enter the password for this computer."
}

private val EyeOpen: ImageVector = materialIcon(name = "PjEye") {
    materialPath {
        moveTo(12f, 4.5f); curveTo(7f, 4.5f, 2.73f, 7.61f, 1f, 12f)
        curveToRelative(1.73f, 4.39f, 6f, 7.5f, 11f, 7.5f); reflectiveCurveToRelative(9.27f, -3.11f, 11f, -7.5f)
        curveToRelative(-1.73f, -4.39f, -6f, -7.5f, -11f, -7.5f); close()
        moveTo(12f, 17f); curveToRelative(-2.76f, 0f, -5f, -2.24f, -5f, -5f); reflectiveCurveToRelative(2.24f, -5f, 5f, -5f)
        reflectiveCurveToRelative(5f, 2.24f, 5f, 5f); reflectiveCurveToRelative(-2.24f, 5f, -5f, 5f); close()
        moveTo(12f, 9f); curveToRelative(-1.66f, 0f, -3f, 1.34f, -3f, 3f); reflectiveCurveToRelative(1.34f, 3f, 3f, 3f)
        reflectiveCurveToRelative(3f, -1.34f, 3f, -3f); reflectiveCurveToRelative(-1.34f, -3f, -3f, -3f); close()
    }
}

private val EyeOff: ImageVector = materialIcon(name = "PjEyeOff") {
    materialPath {
        moveTo(2f, 4.27f); lineTo(3.28f, 3f); lineTo(21f, 20.72f); lineTo(19.73f, 22f); close()
        moveTo(12f, 6.5f); curveToRelative(2.76f, 0f, 5f, 2.24f, 5f, 5f); curveToRelative(0f, 0.65f, -0.13f, 1.26f, -0.36f, 1.83f)
        lineToRelative(2.92f, 2.92f); curveToRelative(1.51f, -1.26f, 2.7f, -2.89f, 3.43f, -4.75f)
        curveToRelative(-1.73f, -4.39f, -6f, -7.5f, -11f, -7.5f); curveToRelative(-1.4f, 0f, -2.74f, 0.25f, -3.98f, 0.7f)
        lineToRelative(2.16f, 2.16f); curveTo(10.74f, 6.13f, 11.35f, 6f, 12f, 6f); close()
        moveTo(1f, 12f); curveToRelative(1.73f, 4.39f, 6f, 7.5f, 11f, 7.5f); curveToRelative(1.55f, 0f, 3.03f, -0.3f, 4.38f, -0.84f)
        lineToRelative(-2.4f, -2.4f); curveToRelative(-0.6f, 0.24f, -1.27f, 0.37f, -1.98f, 0.37f)
        curveToRelative(-2.76f, 0f, -5f, -2.24f, -5f, -5f); curveToRelative(0f, -0.71f, 0.13f, -1.37f, 0.37f, -1.98f)
        lineTo(4.4f, 7.68f); curveTo(2.92f, 8.85f, 1.73f, 10.3f, 1f, 12f); close()
    }
}

/**
 * Compact "Password for <computer>" prompt: short title, one line of context,
 * the password with an eye toggle, a Remember switch, Cancel / Connect, and a
 * small "Ask to connect instead" link. Padded for the keyboard.
 */
@Composable
fun ConnectPasswordDialog(
    computerName: String,
    message: String,
    password: String,
    onPasswordChange: (String) -> Unit,
    remember: Boolean,
    onRememberChange: (Boolean) -> Unit,
    onConnect: () -> Unit,
    onCancel: () -> Unit,
    onAsk: () -> Unit,
) {
    var shown by rememberSaveable { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    Dialog(onDismissRequest = onCancel, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(
            shape = MaterialTheme.shapes.extraLarge,
            tonalElevation = 6.dp,
            modifier = Modifier.padding(horizontal = 24.dp).widthIn(max = 420.dp).fillMaxWidth().imePadding(),
        ) {
            Column(Modifier.padding(start = 20.dp, end = 20.dp, top = 20.dp, bottom = 12.dp)) {
                Text(
                    "Password for $computerName",
                    style = MaterialTheme.typography.titleMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Spacer(Modifier.height(4.dp))
                Text(
                    message,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Spacer(Modifier.height(12.dp))
                OutlinedTextField(
                    value = password,
                    onValueChange = onPasswordChange,
                    modifier = Modifier.fillMaxWidth().focusRequester(focus),
                    singleLine = true,
                    placeholder = { Text("Password") },
                    visualTransformation = if (shown) VisualTransformation.None else PasswordVisualTransformation(),
                    keyboardOptions = submitKeyboard(),
                    keyboardActions = KeyboardActions(onDone = { if (password.isNotBlank()) onConnect() }),
                    trailingIcon = {
                        IconButton(onClick = { shown = !shown }) {
                            Icon(if (shown) EyeOff else EyeOpen, contentDescription = if (shown) "Hide password" else "Show password")
                        }
                    },
                )
                Row(
                    Modifier.fillMaxWidth().padding(top = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("Remember", style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
                    Switch(checked = remember, onCheckedChange = onRememberChange)
                }
                Row(
                    Modifier.fillMaxWidth().padding(top = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    TextButton(onClick = onAsk, contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 4.dp)) {
                        Text("Ask to connect instead", style = MaterialTheme.typography.labelSmall)
                    }
                    Spacer(Modifier.weight(1f))
                    TextButton(onClick = onCancel) { Text("Cancel") }
                    Button(onClick = onConnect, enabled = password.isNotBlank()) { Text("Connect") }
                }
            }
        }
    }
    androidx.compose.runtime.LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
}
