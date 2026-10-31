package us.spader.pjdesk.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.horizontalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.OutlinedTextField
import androidx.compose.ui.unit.sp
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.core.AppLockPolicy
import us.spader.pjdesk.core.DeskState
import us.spader.pjdesk.core.accountStatusLine
import us.spader.pjdesk.core.controlStatus

@Composable
fun SettingsScreen(
    state: DeskState,
    theme: String,
    onTheme: (String) -> Unit,
    startScreen: String = us.spader.pjdesk.core.StartScreen.SAVED,
    onStartScreen: (String) -> Unit = {},
    onServer: (String) -> Unit,
    onResetFingerprint: () -> Unit,
    onSavePermanent: (String, String) -> Boolean = { _, _ -> false },
    onClearPermanent: () -> Unit = {},
    controlEnabled: Boolean = false,
    controlBound: Boolean = false,
    onOpenControl: () -> Unit = {},
    onUnattended: (Boolean) -> Unit = {},
    onUnattendedAccess: (Boolean) -> Unit = {},
    trustedAccounts: List<String> = emptyList(),
    trustedComputers: List<String> = emptyList(),
    onAllowAccount: (String) -> Unit = {},
    onAllowComputer: (String) -> Unit = {},
    onForgetTrusted: (String, String) -> Unit = { _, _ -> },
    onShareLogs: () -> Unit = {},
    onShareLastError: () -> Unit = {},
    onImportConfig: () -> Unit = {},
    onExportConfig: () -> Unit = {},
    importOffer: String? = null,
    importPass: String = "",
    onImportPass: (String) -> Unit = {},
    onAcceptImport: () -> Unit = {},
    onDismissImport: () -> Unit = {},
    computerName: String = "",
    deviceLabel: String = "",
    onComputerName: (String) -> Unit = {},
    tempIdleMinutes: Int = 30,
    onTempIdleMinutes: (Int) -> Unit = {},
    onCheckUpdate: () -> Unit = {},
    signedIn: Boolean = false,
    localPin: Boolean = false,
    hasPin: Boolean = false,
    biometricsOn: Boolean = false,
    biometricsAvailable: Boolean = false,
    autoLockSeconds: Int = 120,
    onChangePin: () -> Unit = {},
    onToggleBiometrics: (Boolean) -> Unit = {},
    onAutoLock: (Int) -> Unit = {},
    onSaveAccountPin: (current: String, password: String, newPin: String, idleSec: Int, enableBio: Boolean) -> Unit = { _, _, _, _, _ -> },
    onRemoveAccountPin: (current: String, password: String) -> Unit = { _, _ -> },
    pinBusy: Boolean = false,
    pinError: String = "",
    engineDetail: String = "",
    accountEmail: String = "",
    accountSession: String = "",
    temporary: Boolean = false,
    onSignIn: (() -> Unit)? = null,
    onSignOut: (() -> Unit)? = null,
    savedCount: Int = 0,
    onRemoveAllSaved: () -> Unit = {},
    onSignOutAndRemove: () -> Unit = {},
    showLoadSaved: Boolean = false,
    onLoadSaved: () -> Unit = {},
    remote: us.spader.pjdesk.core.RemoteOptions = us.spader.pjdesk.core.RemoteOptions(),
    onRemote: (us.spader.pjdesk.core.RemoteOptions) -> Unit = {},
    savedAccessKey: String = "",
    onAccessKey: (String) -> Unit = {},
) {
    var server by rememberSaveable(state.server) { mutableStateOf(state.server) }
    var accessKey by rememberSaveable(savedAccessKey) { mutableStateOf(savedAccessKey) }
    var showAccessKey by rememberSaveable { mutableStateOf(false) }
    var name by rememberSaveable(computerName) { mutableStateOf(computerName) }
    var confirmClear by remember { mutableStateOf(false) }
    var confirmSignOut by remember { mutableStateOf(false) }
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Section("Account") {
            Text(accountStatusLine(accountEmail, accountSession, temporary))
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (onSignIn != null) {
                    Button(onClick = onSignIn) { Text("Sign in") }
                }
                if (onSignOut != null) {
                    OutlinedButton(onClick = onSignOut) { Text("Sign out") }
                }
            }
        }
        Section("Diagnostics") {
            DiagnosticLine("Engine", engineDetail.ifBlank { "Starting PJDesk…" })
            DiagnosticLine("Server", state.server.ifBlank { "Not set" })
            DiagnosticLine("LAN", state.serverLan.ifBlank { "None published" })
            val path = when {
                state.signalError.isNotBlank() -> state.signalError
                state.signalVia.isBlank() -> "Not connected yet"
                else -> "${state.signalVia} ${state.signalReached}".trim()
            }
            DiagnosticLine("Path", path)
            DiagnosticLine(
                "Relay",
                if (state.signalReached.isNotBlank()) state.signalReached else state.server.ifBlank { "Not set" },
            )
            DiagnosticLine("Account", accountStatusLine(accountEmail, accountSession, temporary))
        }
        Section("This phone") {
            OutlinedTextField(
                value = name,
                onValueChange = { name = it.take(64) },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                label = { Text("This phone's name") },
                supportingText = {
                    val shown = deviceLabel.ifBlank { "this phone" }
                    Text("Computer name: $shown. Leave blank to use that. Other computers see this name unless they set a nickname.")
                },
            )
            Button(onClick = { onComputerName(name.trim()) }) { Text("Save name") }
        }
        Section("ID/Relay Server") {
            if (importOffer != null) {
                ServerConfigOffer(
                    blob = importOffer,
                    pass = importPass,
                    onPass = onImportPass,
                    onImport = onAcceptImport,
                    onCancel = onDismissImport,
                    fromPaste = false,
                )
            }
            OutlinedButton(onClick = onImportConfig, modifier = Modifier.fillMaxWidth()) {
                Text("Import server config")
            }
            OutlinedButton(onClick = onExportConfig, modifier = Modifier.fillMaxWidth()) {
                Text("Export server config")
            }
            Text(
                "Import reads a network key from the clipboard and applies it. Export copies this phone's network key. The text is encrypted, so it does not show the server address.",
                color = pjMuted(),
            )
            OutlinedTextField(
                value = server,
                onValueChange = { server = it },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                label = { Text("Address") },
                supportingText = { Text("Filled in when you sign in. Add :port if it is not 21200.") },
            )
            OutlinedTextField(
                value = accessKey,
                onValueChange = { accessKey = it },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
                label = { Text("Access key") },
                visualTransformation = if (showAccessKey) VisualTransformation.None else PasswordVisualTransformation(),
                keyboardOptions = submitKeyboard(),
                keyboardActions = androidx.compose.foundation.text.KeyboardActions(onDone = {
                    onAccessKey(accessKey.trim())
                    onServer(server.trim())
                }),
                trailingIcon = {
                    TextButton(onClick = { showAccessKey = !showAccessKey }) {
                        Text(if (showAccessKey) "Hide" else "Show")
                    }
                },
                supportingText = { Text("From the person who runs the server. This phone can register when sign-in is required.") },
            )
            Button(onClick = {
                onAccessKey(accessKey.trim())
                onServer(server.trim())
            }) { Text("Save server") }
            Text("Key fingerprint", color = pjMuted(), style = MaterialTheme.typography.labelMedium)
            Text(
                state.fingerprint.ifBlank { "Not pinned yet. The first connection saves it." },
                fontFamily = FontFamily.Monospace,
                style = MaterialTheme.typography.bodyMedium,
            )
            TextButton(onClick = onResetFingerprint) { Text("Forget pinned key") }
        }
        Section("Control") {
            Text(controlStatus(controlEnabled, controlBound))
            Button(onClick = onOpenControl, modifier = Modifier.fillMaxWidth()) {
                Text(if (controlEnabled && controlBound) "Open Accessibility" else us.spader.pjdesk.service.ControlNotice.TEXT)
            }
        }
        Section("Security") {
            PermanentPasswordSection(
                hasPermanent = state.hasPermanent,
                onSave = onSavePermanent,
                onRemove = onClearPermanent,
            )
            WhenSomeoneConnects(
                hasPermanent = state.hasPermanent,
                unattended = state.approval == "unattended",
                onUnattended = onUnattended,
            )
            UnattendedAccessRow(on = state.unattendedAccess, onChange = onUnattendedAccess)
            TrustedViewAccess(
                accounts = trustedAccounts,
                computers = trustedComputers,
                onAllowAccount = onAllowAccount,
                onAllowComputer = onAllowComputer,
                onForget = onForgetTrusted,
            )
        }
        Section("Remote control") {
            Text(
                "These are the defaults for every computer. A session can remember its own. The same idle time applies in the foreground, in the background, and when the screen is off. In the foreground, PJDesk warns 10 seconds before it disconnects.",
                color = pjMuted(),
            )
            RemoteOptionsForm(options = remote, onChange = onRemote)
        }
        Section("Display") {
            Text("Theme", color = pjMuted())
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf("system" to "System", "dark" to "Dark", "light" to "Light").forEach { (id, label) ->
                    FilterChip(selected = theme == id, onClick = { onTheme(id) }, label = { Text(label) })
                }
            }
            Text("Open to", color = pjMuted())
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                us.spader.pjdesk.core.StartScreen.choices.forEach { (id, label) ->
                    FilterChip(selected = startScreen == id, onClick = { onStartScreen(id) }, label = { Text(label) })
                }
            }
        }
        if (signedIn || localPin) {
            Section("App lock") {
                Text(
                    if (localPin) {
                        "A PIN is optional on this computer. You joined with a code or a key, so this does not ask for the account password."
                    } else {
                        "PJDesk asks for this PIN when you open it. It is saved to your account as a slow hash, so your other signed-in computers ask for the same PIN. Incoming connections keep working."
                    },
                    color = pjMuted(),
                )
                var editing by rememberSaveable { mutableStateOf(false) }
                var removing by rememberSaveable { mutableStateOf(false) }
                Button(
                    onClick = { editing = true },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text(if (hasPin) "Change PIN" else "Set PIN") }
                if (hasPin) {
                    OutlinedButton(onClick = { removing = true }, modifier = Modifier.fillMaxWidth()) { Text("Remove PIN") }
                }
                if (biometricsAvailable) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                        Text("Unlock with fingerprint or face", modifier = Modifier.weight(1f))
                        Switch(checked = biometricsOn, onCheckedChange = onToggleBiometrics)
                    }
                }
                Text("Lock PJDesk after inactivity", color = pjMuted())
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf(60 to "1 min", 300 to "5 min", 900 to "15 min", 1800 to "30 min", 3600 to "1 hour", 0 to "Never").forEach { (seconds, label) ->
                        FilterChip(
                            selected = autoLockSeconds == seconds,
                            onClick = { onAutoLock(seconds) },
                            label = { Text(label) },
                        )
                    }
                }
                if (editing) {
                    AccountPinDialog(
                        title = if (hasPin) "Change PIN" else "Set PIN",
                        hasPin = hasPin,
                        localOnly = localPin,
                        busy = pinBusy,
                        error = pinError,
                        biometrics = biometricsAvailable,
                        biometricsOn = biometricsOn,
                        onDismiss = { editing = false },
                        onSave = { current, password, pin, bio ->
                            onSaveAccountPin(current, password, pin, autoLockSeconds, bio)
                        },
                    )
                }
                if (removing) {
                    AccountPinDialog(
                        title = "Remove PIN",
                        hasPin = true,
                        localOnly = localPin,
                        busy = pinBusy,
                        error = pinError,
                        biometrics = false,
                        biometricsOn = false,
                        remove = true,
                        onDismiss = { removing = false },
                        onSave = { current, password, _, _ ->
                            onRemoveAccountPin(current, password)
                        },
                    )
                }
            }
        }
        Section("Sign-in") {
            Text(
                if (showLoadSaved) "Saved computers stay on this phone until you load them."
                else "Saved computers sync with your account. Saved passwords sync too, encrypted so the server cannot read them.",
                color = pjMuted(),
            )
            if (showLoadSaved) {
                Button(onClick = onLoadSaved, modifier = Modifier.fillMaxWidth()) {
                    Text("Load my saved computers")
                }
            }
            if (onSignOut != null) {
                Button(onClick = onSignOut, modifier = Modifier.fillMaxWidth()) { Text("Sign out") }
            }
            Text("Temporary sign-in", style = MaterialTheme.typography.titleMedium)
            Text(
                "Minutes with no PJDesk activity before a temporary sign-in ends. The sign-in checkbox starts unchecked, so this phone stays signed in.",
                color = pjMuted(),
            )
            var idle by rememberSaveable(tempIdleMinutes) { mutableStateOf(tempIdleMinutes.toString()) }
            OutlinedTextField(
                value = idle,
                onValueChange = {
                    idle = it.filter { ch -> ch.isDigit() }.take(4)
                    idle.toIntOrNull()?.let { n -> onTempIdleMinutes(n.coerceIn(1, 24 * 60)) }
                },
                singleLine = true,
                label = { Text("Idle minutes") },
            )
        }
        Section("Privacy") {
            Text(
                "Removing saved computers here does not delete them from your pjdesk.com account or from your other computers.",
                color = pjMuted(),
            )
            OutlinedButton(onClick = { confirmClear = true }, modifier = Modifier.fillMaxWidth()) {
                Text("Remove all saved computers from this computer")
            }
            OutlinedButton(onClick = { confirmSignOut = true }, modifier = Modifier.fillMaxWidth()) {
                Text("Sign out and remove everything from this computer")
            }
            Text(
                "Sign out and remove also ends the pjdesk.com sign-in on this computer. When Sign me out when I'm done is on, closing PJDesk clears the saved list, cached passwords, and recent computers.",
                color = pjMuted(),
            )
        }
        if (confirmClear) {
            LocalSavedClearDialog(
                title = "Remove all saved computers",
                count = savedCount,
                detail = "This clears the saved list, cached passwords, recent computers, and saved addresses on this computer only. Nothing is deleted from your pjdesk.com account or from your other computers.",
                confirm = "Remove from this computer",
                onDismiss = { confirmClear = false },
                onConfirm = {
                    confirmClear = false
                    onRemoveAllSaved()
                },
            )
        }
        if (confirmSignOut) {
            LocalSavedClearDialog(
                title = "Sign out and remove everything",
                count = savedCount,
                detail = "This signs out on this computer and clears the saved list, cached passwords, recent computers, and saved addresses here. Nothing is deleted from your pjdesk.com account or from your other computers.",
                confirm = "Sign out and remove",
                onDismiss = { confirmSignOut = false },
                onConfirm = {
                    confirmSignOut = false
                    onSignOutAndRemove()
                },
            )
        }
        Section("About") {
            Text("PJDesk 0.6.78", style = MaterialTheme.typography.titleMedium)
            OutlinedButton(onClick = onCheckUpdate, modifier = Modifier.fillMaxWidth()) {
                Text("Check for updates")
            }
            Text(
                "Android host and controller. Video is H.264 Annex-B from the hardware encoder, the same stream the Windows client decodes with OpenH264. The server comes from your PJDesk account after you sign in.",
                color = pjMuted(),
            )
            TextButton(onClick = onShareLogs) { Text("Share logs") }
            TextButton(onClick = onShareLastError) { Text("Share last error") }
        }
    }
}

@Composable
private fun AccountPinDialog(
    title: String,
    hasPin: Boolean,
    localOnly: Boolean = false,
    busy: Boolean,
    error: String,
    biometrics: Boolean,
    biometricsOn: Boolean,
    remove: Boolean = false,
    onDismiss: () -> Unit,
    onSave: (current: String, password: String, pin: String, enableBio: Boolean) -> Unit,
) {
    var current by remember { mutableStateOf("") }
    var password by remember { mutableStateOf("") }
    var pin by remember { mutableStateOf("") }
    var confirm by remember { mutableStateOf("") }
    var showPin by remember { mutableStateOf(false) }
    var enableBio by remember { mutableStateOf(biometricsOn) }
    var localError by remember { mutableStateOf("") }
    var submitted by remember { mutableStateOf(false) }
    val pinFocus = remember { FocusRequester() }
    val confirmFocus = remember { FocusRequester() }
    fun save() {
        localError = when {
            localOnly && hasPin && current.isBlank() -> "Enter the current PIN."
            !localOnly && hasPin && current.isBlank() && password.isBlank() -> "Enter the current PIN or your account password."
            !localOnly && !hasPin && password.isBlank() -> "Enter your account password."
            !remove && !AppLockPolicy.pinAcceptable(pin) -> "Use 4 to 8 digits."
            !remove && pin != confirm -> "Those PINs do not match."
            else -> ""
        }
        if (localError.isEmpty()) {
            submitted = true
            onSave(current, password, pin, enableBio && biometrics)
        }
    }
    LaunchedEffect(busy, error, submitted) {
        if (submitted && !busy && error.isEmpty()) onDismiss()
    }
    LaunchedEffect(Unit) {
        if (!hasPin && !remove) runCatching { pinFocus.requestFocus() }
    }
    val shown = localError.ifBlank { error }
    AlertDialog(
        onDismissRequest = { if (!busy) onDismiss() },
        title = { Text(title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    when {
                        localOnly -> "Choose a PIN for this computer. This does not ask for an account password."
                        hasPin -> "Enter the current PIN or your account password."
                        else -> "Enter your account password."
                    },
                    color = pjMuted(),
                )
                if (hasPin) {
                    PinEntry(current, { current = it }, "Current PIN", showPin, { showPin = !showPin }, ImeAction.Next) {
                        if (!remove) runCatching { pinFocus.requestFocus() } else save()
                    }
                }
                if (!localOnly) {
                    AccountPasswordField(
                        value = password,
                        onValueChange = { password = it },
                        label = "Account password",
                        autofocus = !hasPin,
                        onSubmit = { if (remove) save() else runCatching { pinFocus.requestFocus() } },
                    )
                }
                if (!remove) {
                    PinEntry(
                        pin,
                        { pin = it },
                        "New PIN",
                        showPin,
                        { showPin = !showPin },
                        ImeAction.Next,
                        Modifier.focusRequester(pinFocus),
                    ) { runCatching { confirmFocus.requestFocus() } }
                    PinEntry(
                        confirm,
                        { confirm = it },
                        "Confirm PIN",
                        showPin,
                        { showPin = !showPin },
                        ImeAction.Done,
                        Modifier.focusRequester(confirmFocus),
                    ) { save() }
                    if (biometrics) {
                        Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                            androidx.compose.material3.Checkbox(checked = enableBio, onCheckedChange = { enableBio = it })
                            Text("Unlock with fingerprint or face")
                        }
                    }
                }
                if (shown.isNotEmpty()) Text(shown, color = MaterialTheme.colorScheme.error)
            }
        },
        confirmButton = {
            TextButton(enabled = !busy, onClick = { save() }) {
                Text(if (busy) "Saving…" else if (remove) "Remove" else "Save")
            }
        },
        dismissButton = { TextButton(onClick = onDismiss, enabled = !busy) { Text("Cancel") } },
    )
}

@Composable
private fun PinEntry(
    value: String,
    onValue: (String) -> Unit,
    label: String,
    shown: Boolean,
    onToggle: () -> Unit,
    ime: ImeAction = ImeAction.Next,
    modifier: Modifier = Modifier,
    onIme: () -> Unit = {},
) {
    OutlinedTextField(
        value = value,
        onValueChange = { onValue(it.filter { ch -> ch.isDigit() }.take(8)) },
        label = { Text(label) },
        singleLine = true,
        visualTransformation = if (shown) VisualTransformation.None else PasswordVisualTransformation(),
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword, imeAction = ime),
        keyboardActions = KeyboardActions(onNext = { onIme() }, onDone = { onIme() }),
        trailingIcon = { TextButton(onClick = onToggle) { Text(if (shown) "Hide" else "Show") } },
        textStyle = LocalTextStyle.current.copy(textAlign = TextAlign.Center, letterSpacing = 4.sp),
        modifier = modifier.fillMaxWidth(),
    )
}

@Composable
private fun DiagnosticLine(label: String, value: String) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(label, color = pjMuted(), style = MaterialTheme.typography.labelMedium)
        Text(value, style = MaterialTheme.typography.bodyLarge)
    }
}

@Composable
private fun Section(title: String, content: @Composable () -> Unit) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            content()
        }
    }
}
