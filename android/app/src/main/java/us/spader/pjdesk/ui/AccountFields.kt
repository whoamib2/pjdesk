package us.spader.pjdesk.ui

import android.text.InputType
import android.view.View
import android.view.inputmethod.EditorInfo
import android.widget.EditText
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.widget.doAfterTextChanged

/** Email field password managers can fill. Compose 1.7 has no autofill content type, so this is a real EditText. */
@Composable
fun AccountEmailField(
    value: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    label: String = "Email",
) {
    val current = rememberUpdatedState(value)
    val notify = rememberUpdatedState(onValueChange)
    AndroidView(
        modifier = modifier.fillMaxWidth(),
        factory = { context ->
            accountEdit(context, label).apply {
                setAutofillHints(View.AUTOFILL_HINT_USERNAME, View.AUTOFILL_HINT_EMAIL_ADDRESS)
                inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_EMAIL_ADDRESS
                imeOptions = EditorInfo.IME_ACTION_NEXT
                doAfterTextChanged { text ->
                    val next = text?.toString().orEmpty()
                    if (next != current.value) notify.value(next)
                }
            }
        },
        update = { edit ->
            if (edit.text.toString() != value) {
                edit.setText(value)
                edit.setSelection(edit.text.length)
            }
        },
    )
}

/** Account password field. Show keeps the password autofill hint. */
@Composable
fun AccountPasswordField(
    value: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    label: String = "Password",
) {
    var shown by remember { mutableStateOf(false) }
    val current = rememberUpdatedState(value)
    val notify = rememberUpdatedState(onValueChange)
    Row(modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        AndroidView(
            modifier = Modifier.weight(1f),
            factory = { context ->
                accountEdit(context, label).apply {
                    setAutofillHints(View.AUTOFILL_HINT_PASSWORD)
                    imeOptions = EditorInfo.IME_ACTION_DONE
                    doAfterTextChanged { text ->
                        val next = text?.toString().orEmpty()
                        if (next != current.value) notify.value(next)
                    }
                }
            },
            update = { edit ->
                edit.setAutofillHints(View.AUTOFILL_HINT_PASSWORD)
                val type = if (shown) {
                    InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD
                } else {
                    InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
                }
                if (edit.inputType != type) edit.inputType = type
                if (edit.text.toString() != value) {
                    edit.setText(value)
                    edit.setSelection(edit.text.length)
                }
                edit.hint = label
            },
        )
        TextButton(onClick = { shown = !shown }) { Text(if (shown) "Hide" else "Show") }
    }
}

private fun accountEdit(context: android.content.Context, label: String): EditText =
    EditText(context).apply {
        importantForAutofill = View.IMPORTANT_FOR_AUTOFILL_YES
        setSingleLine(true)
        hint = label
        setPadding(32, 28, 32, 28)
    }
