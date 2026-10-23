package us.spader.pjdesk.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.core.DeskState
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.computerTitle

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
    recent: List<SavedComputer> = emptyList(),
    onRecent: (String) -> Unit = {},
) {
    var showPassword by rememberSaveable { mutableStateOf(false) }
    val starting = state.status.startsWith("Starting PJDesk") ||
        state.status.startsWith("Connecting") ||
        state.status.startsWith("Getting your PJDesk server")
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Column(Modifier.weight(1f)) {
                Text(
                    when {
                        accountEmail.isBlank() -> "Not signed in"
                        temporary -> "Signed in until you close PJDesk"
                        else -> accountEmail
                    },
                    style = MaterialTheme.typography.titleMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (starting) CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
                    Text(state.status, color = pjMuted(), style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
            if (onSignIn != null) {
                Button(onClick = onSignIn) { Text("Sign in") }
            }
        }
        if (state.hosting) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Someone is viewing this phone.", Modifier.weight(1f), color = pjMuted())
                OutlinedButton(onClick = onStop) { Text("Stop") }
            }
        }
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Your ID", color = pjMuted(), style = MaterialTheme.typography.labelMedium, modifier = Modifier.weight(1f))
                    IconButton(onClick = onCopyId, enabled = state.id.isNotEmpty()) { Glyph("copy") }
                }
                Text(
                    state.idFormatted.ifBlank { "Waiting for the server" },
                    style = MaterialTheme.typography.headlineMedium,
                    fontFamily = FontFamily.Monospace,
                    maxLines = 1,
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        if (showPassword) state.password.ifBlank { "—" } else "•".repeat(state.password.length.coerceAtLeast(8)),
                        modifier = Modifier.weight(1f),
                        style = MaterialTheme.typography.titleLarge,
                        fontFamily = FontFamily.Monospace,
                        maxLines = 1,
                    )
                    IconButton(onClick = { showPassword = !showPassword }, enabled = state.password.isNotEmpty()) {
                        Glyph(if (showPassword) "hide" else "show")
                    }
                    IconButton(onClick = onCopyPassword, enabled = state.password.isNotEmpty()) { Glyph("copy") }
                    IconButton(onClick = onRegenerate) { Glyph("refresh") }
                }
                Text("One-time password", color = pjMuted(), style = MaterialTheme.typography.labelMedium)
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(
                value = connectId.filter { it.isDigit() }.take(9),
                onValueChange = { onConnectId(it.filter { ch -> ch.isDigit() }.take(9)) },
                modifier = Modifier.weight(1f),
                singleLine = true,
                label = { Text(if (connectName.isBlank()) "Partner ID" else connectName) },
                visualTransformation = IdGroupTransformation,
                textStyle = androidx.compose.ui.text.TextStyle(fontFamily = FontFamily.Monospace),
            )
            Button(onClick = onConnect, modifier = Modifier.widthIn(min = 108.dp)) { Text("Connect") }
        }
        if (recent.isNotEmpty()) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                recent.take(3).forEach { computer ->
                    FilterChip(
                        selected = false,
                        onClick = { onRecent(computer.id.filter { it.isDigit() }.take(9)) },
                        label = {
                            Text(computerTitle(computer), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        },
                    )
                }
            }
        }
    }
}

@Composable
private fun Glyph(kind: String) {
    val c = MaterialTheme.colorScheme.onSurface
    androidx.compose.foundation.Canvas(Modifier.size(22.dp)) {
        val paint = androidx.compose.ui.graphics.drawscope.Stroke(width = 2.dp.toPx())
        when (kind) {
            "copy" -> {
                drawRoundRect(c, topLeft = androidx.compose.ui.geometry.Offset(size.width * 0.28f, size.height * 0.18f), size = androidx.compose.ui.geometry.Size(size.width * 0.5f, size.height * 0.58f), cornerRadius = androidx.compose.ui.geometry.CornerRadius(3f, 3f), style = paint)
                drawRoundRect(c, topLeft = androidx.compose.ui.geometry.Offset(size.width * 0.16f, size.height * 0.32f), size = androidx.compose.ui.geometry.Size(size.width * 0.5f, size.height * 0.5f), cornerRadius = androidx.compose.ui.geometry.CornerRadius(3f, 3f), style = paint)
            }
            "show", "hide" -> {
                drawOval(c, topLeft = androidx.compose.ui.geometry.Offset(size.width * 0.1f, size.height * 0.32f), size = androidx.compose.ui.geometry.Size(size.width * 0.8f, size.height * 0.36f), style = paint)
                drawCircle(c, radius = size.minDimension * 0.08f, center = center)
                if (kind == "hide") {
                    drawLine(c, androidx.compose.ui.geometry.Offset(size.width * 0.18f, size.height * 0.78f), androidx.compose.ui.geometry.Offset(size.width * 0.82f, size.height * 0.22f), strokeWidth = 2.dp.toPx())
                }
            }
            else -> {
                drawArc(c, startAngle = 40f, sweepAngle = 280f, useCenter = false, topLeft = androidx.compose.ui.geometry.Offset(size.width * 0.2f, size.height * 0.2f), size = androidx.compose.ui.geometry.Size(size.width * 0.6f, size.height * 0.6f), style = paint)
                drawLine(c, androidx.compose.ui.geometry.Offset(size.width * 0.5f, size.height * 0.12f), androidx.compose.ui.geometry.Offset(size.width * 0.5f, size.height * 0.32f), strokeWidth = 2.dp.toPx())
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
