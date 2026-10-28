package us.spader.pjdesk.ui

import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation

@Composable
fun SecretField(
    value: String,
    onValueChange: (String) -> Unit,
    label: String,
    modifier: Modifier = Modifier,
    readOnly: Boolean = false,
    onSubmit: (() -> Unit)? = null,
) {
    var shown by rememberSaveable(label) { mutableStateOf(false) }
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        modifier = modifier.fillMaxWidth(),
        readOnly = readOnly,
        singleLine = true,
        label = { Text(label) },
        visualTransformation = if (shown) VisualTransformation.None else PasswordVisualTransformation(),
        keyboardOptions = if (onSubmit != null) submitKeyboard() else KeyboardOptions(keyboardType = KeyboardType.Password),
        keyboardActions = KeyboardActions(onDone = { onSubmit?.invoke() }),
        trailingIcon = {
            TextButton(onClick = { shown = !shown }) {
                Text(if (shown) "Hide" else "Show")
            }
        },
    )
}

/**
 * Keyboard for a password or PIN field whose Enter (the IME action) submits,
 * just like tapping the button under it.
 */
fun submitKeyboard(type: KeyboardType = KeyboardType.Password): KeyboardOptions =
    KeyboardOptions(keyboardType = type, imeAction = ImeAction.Done)
