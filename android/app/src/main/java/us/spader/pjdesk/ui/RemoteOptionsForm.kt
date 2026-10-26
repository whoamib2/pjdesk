package us.spader.pjdesk.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.core.RemoteOptions
import kotlin.math.roundToInt

@Composable
fun RemoteOptionsForm(
    options: RemoteOptions,
    onChange: (RemoteOptions) -> Unit,
    permissions: Int = -1,
    peerName: String = "",
    forThisComputer: Boolean = false,
    onForThisComputer: ((Boolean) -> Unit)? = null,
    showActions: Boolean = false,
    fileNames: List<String> = emptyList(),
    onClipboardNow: () -> Unit = {},
    onFileList: () -> Unit = {},
    onSendFile: () -> Unit = {},
    onRestart: () -> Unit = {},
    onLock: () -> Unit = {},
    onCtrlAltDel: () -> Unit = {},
    onDisconnect: () -> Unit = {},
) {
    val current = options.cleaned()
    fun edit(next: RemoteOptions) = onChange(next.cleaned())
    var confirmRestart by remember { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        if (onForThisComputer != null) {
            ToggleRow(
                label = "Only for this computer",
                checked = forThisComputer,
                detail = if (peerName.isBlank()) {
                    "On, this session keeps its own copy. Off, changes are the default for every computer."
                } else {
                    "On, this session keeps its own copy for $peerName. Off, changes are the default for every computer."
                },
                onChange = onForThisComputer,
            )
        }
        Text("Input", style = MaterialTheme.typography.titleMedium)
        ChipRow {
            listOf("auto" to "Auto", "touch" to "Touch", "trackpad" to "Mouse").forEach { (id, label) ->
                FilterChip(selected = current.inputMode == id, onClick = { edit(current.copy(inputMode = id)) }, label = { Text(label) })
            }
        }
        Text("Pointer speed ${speedLabel(current.pointerSpeed)}", color = pjMuted())
        Slider(
            value = current.pointerSpeed,
            onValueChange = { edit(current.copy(pointerSpeed = it)) },
            valueRange = 0.25f..2f,
        )
        Text("Scroll speed ${speedLabel(current.scrollSpeed)}", color = pjMuted())
        Slider(
            value = current.scrollSpeed,
            onValueChange = { edit(current.copy(scrollSpeed = it)) },
            valueRange = 0.25f..4f,
        )
        ToggleRow("Invert scroll", current.invertScroll) { edit(current.copy(invertScroll = it)) }
        ToggleRow("Pointer acceleration", current.pointerAccel) { edit(current.copy(pointerAccel = it)) }
        ToggleRow("Drag lock", current.dragLock, detail = "A one-finger drag holds the mouse button.") {
            edit(current.copy(dragLock = it))
        }
        ToggleRow("Show local cursor", current.showLocalCursor) { edit(current.copy(showLocalCursor = it)) }
        ToggleRow(
            "Show remote cursor",
            checked = true,
            enabled = false,
            detail = "The other computer draws its cursor into the picture.",
            onChange = {},
        )
        Text("Display", style = MaterialTheme.typography.titleMedium)
        ChipRow {
            listOf("fit" to "Fit", "original" to "Original", "stretch" to "Stretch").forEach { (id, label) ->
                FilterChip(selected = current.scale == id, onClick = { edit(current.copy(scale = id)) }, label = { Text(label) })
            }
        }
        if (showActions) {
            Text("Image quality", color = pjMuted())
        }
        ChipRow {
            listOf("best" to "Best", "balanced" to "Balanced", "speed" to "Speed", "custom" to "Custom").forEach { (id, label) ->
                FilterChip(selected = current.quality == id, onClick = { edit(current.copy(quality = id)) }, label = { Text(label) })
            }
        }
        if (current.quality == "custom") {
            Text("Frame rate ${current.customFps}", color = pjMuted())
            Slider(
                value = current.customFps.toFloat(),
                onValueChange = { edit(current.copy(customFps = it.roundToInt())) },
                valueRange = 5f..60f,
            )
            Text("Quality ${current.customBitrate} kbps", color = pjMuted())
            Slider(
                value = current.customBitrate.toFloat(),
                onValueChange = { edit(current.copy(customBitrate = (it / 100f).roundToInt() * 100)) },
                valueRange = 200f..20_000f,
            )
        }
        ToggleRow("View only", current.viewOnly, detail = "Watch the screen without sending input.") {
            edit(current.copy(viewOnly = it))
        }
        ToggleRow("Show quality monitor", current.showStats) { edit(current.copy(showStats = it)) }
        val privacyOk = RemoteOptions.allows(permissions, RemoteOptions.PERM_PRIVACY)
        ToggleRow(
            "Blank the other screen",
            current.privacy,
            enabled = privacyOk,
            detail = if (privacyOk) "Privacy mode, while this session is connected." else "Not allowed for this session.",
        ) { edit(current.copy(privacy = it)) }
        Text("Keyboard", style = MaterialTheme.typography.titleMedium)
        ToggleRow("Show keyboard", current.showKeyBar) { edit(current.copy(showKeyBar = it)) }
        Text("Keyboard mode", color = pjMuted())
        ChipRow {
            listOf("map" to "Map", "translate" to "Translate").forEach { (id, label) ->
                FilterChip(selected = current.keyboardMode == id, onClick = { edit(current.copy(keyboardMode = id)) }, label = { Text(label) })
            }
        }
        if (showActions) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = onCtrlAltDel) { Text("Ctrl+Alt+Del") }
                OutlinedButton(onClick = onLock) { Text("Lock") }
            }
        }
        Text("Session", style = MaterialTheme.typography.titleMedium)
        val clipOk = RemoteOptions.allows(permissions, RemoteOptions.PERM_CLIPBOARD)
        ToggleRow(
            "Clipboard sync",
            current.clipboardSync,
            enabled = clipOk,
            detail = if (clipOk) "Copy on either side shows up on the other." else "Not allowed for this session.",
        ) { edit(current.copy(clipboardSync = it)) }
        if (showActions) {
            OutlinedButton(onClick = onClipboardNow, enabled = clipOk) { Text("Send clipboard now") }
        }
        val audioOk = RemoteOptions.allows(permissions, RemoteOptions.PERM_AUDIO)
        ToggleRow(
            "Remote audio",
            checked = false,
            enabled = false,
            detail = if (audioOk) "This phone does not play remote audio yet." else "Not allowed for this session.",
            onChange = {},
        )
        if (showActions) {
            val filesOk = RemoteOptions.allows(permissions, RemoteOptions.PERM_FILE)
            Text(if (filesOk) "Files go to that computer's receive folder." else "File transfer is not allowed for this session.", color = pjMuted())
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = onFileList, enabled = filesOk) { Text("Their folder") }
                OutlinedButton(onClick = onSendFile, enabled = filesOk) { Text("Send a file") }
            }
            if (fileNames.isNotEmpty()) {
                Text(fileNames.joinToString("\n"), style = MaterialTheme.typography.bodySmall)
            }
            val restartOk = RemoteOptions.allows(permissions, RemoteOptions.PERM_RESTART)
            OutlinedButton(onClick = { confirmRestart = true }, enabled = restartOk) { Text("Restart remote") }
            if (!restartOk) Text("Restart is not allowed for this session.", color = pjMuted())
        }
        Text("End a remote session after no input. The same time applies in the foreground, in the background, and when the screen is off.", color = pjMuted())
        ChipRow {
            listOf(0 to "Off", 1 to "1 min", 5 to "5 min", 15 to "15 min").forEach { (minutes, label) ->
                FilterChip(
                    selected = current.sessionIdleMin == minutes,
                    onClick = { edit(current.copy(sessionIdleMin = minutes)) },
                    label = { Text(label) },
                )
            }
        }
        if (showActions) {
            Button(onClick = onDisconnect, modifier = Modifier.fillMaxWidth()) { Text("Disconnect") }
        }
    }
    if (confirmRestart) {
        AlertDialog(
            onDismissRequest = { confirmRestart = false },
            title = { Text("Restart ${peerName.ifBlank { "the other computer" }}?") },
            text = { Text("That computer will reboot if this session is allowed to restart it.") },
            confirmButton = {
                TextButton(onClick = { confirmRestart = false; onRestart() }) { Text("Restart") }
            },
            dismissButton = {
                TextButton(onClick = { confirmRestart = false }) { Text("Cancel") }
            },
        )
    }
}

@Composable
private fun ChipRow(content: @Composable () -> Unit) {
    Row(
        Modifier.horizontalScroll(rememberScrollState()),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        content()
    }
}

@Composable
private fun ToggleRow(
    label: String,
    checked: Boolean,
    enabled: Boolean = true,
    detail: String? = null,
    onChange: (Boolean) -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 48.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).padding(end = 8.dp)) {
            Text(label)
            if (!detail.isNullOrBlank()) {
                Text(detail, color = pjMuted(), style = MaterialTheme.typography.bodySmall)
            }
        }
        Switch(checked = checked, onCheckedChange = onChange, enabled = enabled)
    }
}

private fun speedLabel(speed: Float): String {
    val shown = (speed * 10f).roundToInt() / 10f
    return if (shown % 1f == 0f) "${shown.toInt()}×" else "${shown}×"
}
