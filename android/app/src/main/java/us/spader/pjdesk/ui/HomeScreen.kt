package us.spader.pjdesk.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import kotlinx.coroutines.delay
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
import us.spader.pjdesk.core.accountStatusLine
import us.spader.pjdesk.service.ControlNotice

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
    accountSession: String = "",
    onSignOut: (() -> Unit)? = null,
    temporary: Boolean = false,
    recent: List<SavedComputer> = emptyList(),
    onRecent: (String) -> Unit = {},
    selfId: String = "",
    controlEnabled: Boolean = true,
    controlBound: Boolean = true,
    onOpenControl: () -> Unit = {},
    onRefreshSupport: () -> Unit = {},
    onSupportConnect: (String, String) -> Unit = { _, _ -> },
    inviteCode: String = "",
    inviteExpiresAt: Long = 0,
    inviteId: String = "",
    inviteBusy: Boolean = false,
    inviteError: String = "",
    onCreateInvite: () -> Unit = {},
    onRevokeInvite: () -> Unit = {},
    onIncomingOnly: (Boolean, String) -> Unit = { _, _ -> },
) {
    var showPassword by rememberSaveable { mutableStateOf(false) }
    var showSupport by rememberSaveable { mutableStateOf(false) }
    var helpCode by rememberSaveable { mutableStateOf("") }
    var helpPass by rememberSaveable { mutableStateOf("") }
    var helpShow by rememberSaveable { mutableStateOf(false) }
    var ownerPass by rememberSaveable { mutableStateOf("") }
    var ownerShow by rememberSaveable { mutableStateOf(false) }
    var inviteOpen by rememberSaveable { mutableStateOf(false) }
    var tick by remember { mutableLongStateOf(System.currentTimeMillis() / 1000) }
    LaunchedEffect(inviteOpen) {
        while (inviteOpen) {
            tick = System.currentTimeMillis() / 1000
            delay(1000)
        }
    }
    val starting = state.status.startsWith("Starting PJDesk") ||
        state.status.startsWith("Connecting") ||
        state.status.startsWith("Getting your PJDesk server")
    val self = selfId.filter { it.isDigit() }
    Column(
        modifier = Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 16.dp, vertical = 4.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(
                accountStatusLine(accountEmail, accountSession, temporary),
                style = MaterialTheme.typography.titleMedium,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (onSignIn != null) {
                    Button(onClick = onSignIn) { Text("Sign in") }
                }
                if (onSignOut != null) {
                    OutlinedButton(onClick = onSignOut) { Text("Sign out") }
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (starting) CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
                Text(state.status, color = pjMuted(), style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        if (state.hosting) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Someone is viewing this phone.", Modifier.weight(1f), color = pjMuted())
                OutlinedButton(onClick = onStop) { Text("Stop") }
            }
        }
        if (!(controlEnabled && controlBound)) {
            Card(
                onClick = onOpenControl,
                modifier = Modifier.fillMaxWidth(),
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer),
            ) {
                Text(
                    ControlNotice.TEXT,
                    modifier = Modifier.padding(12.dp),
                    color = MaterialTheme.colorScheme.onErrorContainer,
                )
            }
        }
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(horizontal = 12.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("Let someone connect", style = MaterialTheme.typography.titleMedium)
                if (state.server.isBlank() || state.id.length != 9) {
                    Text("This phone is not on a network yet, so a support code cannot be used.", color = pjMuted())
                } else if (state.supportCode.length == 8) {
                    Text(
                        state.supportCode.chunked(4).joinToString(" "),
                        style = MaterialTheme.typography.headlineMedium,
                        fontFamily = FontFamily.Monospace,
                    )
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            if (showSupport) state.supportPassword.ifBlank { "—" }
                            else "•".repeat(state.supportPassword.length.coerceAtLeast(8)),
                            modifier = Modifier.weight(1f),
                            fontFamily = FontFamily.Monospace,
                            maxLines = 1,
                        )
                        IconButton(onClick = { showSupport = !showSupport }, enabled = state.supportPassword.isNotEmpty()) {
                            Glyph(if (showSupport) "hide" else "show")
                        }
                        IconButton(onClick = onRefreshSupport) { Glyph("refresh") }
                    }
                    Text("Temporary password", color = pjMuted(), style = MaterialTheme.typography.labelMedium)
                    Text(
                        "About 10 minutes if unused, then a new code. This is not the permanent password. PJDesk will not ask Allow. Android may still ask to share the screen. This phone still shows who connected.",
                        color = pjMuted(),
                        style = MaterialTheme.typography.bodySmall,
                    )
                } else {
                    Text("Preparing a code", color = pjMuted())
                }
            }
        }
        if (accountEmail.isNotBlank()) {
            OutlinedButton(onClick = { inviteOpen = !inviteOpen }, modifier = Modifier.fillMaxWidth()) {
                Text(if (inviteOpen) "Hide invite" else "Invite someone")
            }
            if (inviteOpen) {
                Card(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        if (inviteCode.length == 8) {
                            Text(
                                inviteCode.chunked(4).joinToString(" "),
                                style = MaterialTheme.typography.headlineMedium,
                                fontFamily = FontFamily.Monospace,
                            )
                            Text(
                                "Expires in ${inviteLeft(inviteExpiresAt, tick)}. One use. They do not need a PJDesk account.",
                                color = pjMuted(),
                            )
                        } else if (inviteId.isNotBlank()) {
                            Text(
                                "A code is already active and expires in ${inviteLeft(inviteExpiresAt, tick)}. The digits were shown once.",
                                color = pjMuted(),
                            )
                        } else {
                            Text(
                                "Create an 8-digit code. It works once, lasts one hour, and the other person does not need a PJDesk account.",
                                color = pjMuted(),
                            )
                        }
                        if (inviteError.isNotBlank()) Text(inviteError, color = MaterialTheme.colorScheme.error)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Button(onClick = onCreateInvite, enabled = !inviteBusy) {
                                Text(if (inviteCode.isEmpty() && inviteId.isEmpty()) "Create code" else "Create a new code")
                            }
                            if (inviteId.isNotBlank()) {
                                OutlinedButton(onClick = onRevokeInvite, enabled = !inviteBusy) { Text("Revoke") }
                            }
                        }
                    }
                }
            }
        }
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(horizontal = 12.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
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
        Row(verticalAlignment = Alignment.CenterVertically) {
            Checkbox(
                checked = state.incomingOnly,
                onCheckedChange = { checked ->
                    if (checked) onIncomingOnly(true, "")
                },
            )
            Text("Incoming only (this computer can't connect to others)", style = MaterialTheme.typography.bodyMedium)
        }
        if (state.incomingOnly) {
            Text(
                "Incoming only is on. This computer can't connect to others.",
                color = pjMuted(),
                style = MaterialTheme.typography.bodyMedium,
            )
            OutlinedTextField(
                value = ownerPass,
                onValueChange = { ownerPass = it },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                label = { Text("Account owner's password") },
                visualTransformation = if (ownerShow) VisualTransformation.None else androidx.compose.ui.text.input.PasswordVisualTransformation(),
                trailingIcon = {
                    IconButton(onClick = { ownerShow = !ownerShow }) { Glyph(if (ownerShow) "hide" else "show") }
                },
            )
            Button(
                onClick = {
                    onIncomingOnly(false, ownerPass)
                    ownerPass = ""
                },
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Allow outgoing") }
        } else {
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
        OutlinedTextField(
            value = helpCode.filter { it.isDigit() }.take(8),
            onValueChange = { helpCode = it.filter { ch -> ch.isDigit() }.take(8) },
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            label = { Text("Their support code") },
            placeholder = { Text("1234 5678") },
            textStyle = androidx.compose.ui.text.TextStyle(fontFamily = FontFamily.Monospace),
        )
        OutlinedTextField(
            value = helpPass,
            onValueChange = { helpPass = it },
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            label = { Text("Temporary password") },
            visualTransformation = if (helpShow) VisualTransformation.None else androidx.compose.ui.text.input.PasswordVisualTransformation(),
            trailingIcon = {
                IconButton(onClick = { helpShow = !helpShow }) { Glyph(if (helpShow) "hide" else "show") }
            },
        )
        OutlinedButton(
            onClick = { onSupportConnect(helpCode.filter { it.isDigit() }.take(8), helpPass) },
            modifier = Modifier.fillMaxWidth(),
        ) { Text("Connect with code") }
        if (recent.isNotEmpty()) {
            Text("Recent", style = MaterialTheme.typography.titleMedium)
            Column(Modifier.fillMaxWidth()) {
                recent.take(3).forEach { computer ->
                    val mine = self.isNotBlank() && computer.id.filter { it.isDigit() } == self
                    ComputerRow(
                        computer = computer,
                        mine = mine,
                        showMenu = false,
                        inset = 0.dp,
                        onConnect = { onRecent(computer.id.filter { it.isDigit() }.take(9)) },
                    )
                }
            }
        }
        }
    }
}

private fun inviteLeft(expiresAt: Long, now: Long): String {
    if (expiresAt <= now) return "0 s"
    val left = expiresAt - now
    val hours = left / 3600
    val mins = (left % 3600) / 60
    val secs = left % 60
    return if (hours > 0) "$hours h $mins min" else if (mins > 0) "$mins min $secs s" else "$secs s"
}

@Composable
private fun Glyph(kind: String) {
    val icon = when (kind) {
        "copy" -> PjIcons.Copy
        "show" -> PjIcons.Show
        "hide" -> PjIcons.Hide
        else -> PjIcons.Refresh
    }
    val description = when (kind) {
        "copy" -> "Copy"
        "show" -> "Show"
        "hide" -> "Hide"
        else -> "Refresh"
    }
    Icon(icon, contentDescription = description, tint = MaterialTheme.colorScheme.onSurface)
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
