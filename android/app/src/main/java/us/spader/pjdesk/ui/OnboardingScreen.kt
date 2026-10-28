package us.spader.pjdesk.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

@Composable
fun OnboardingScreen(
    notificationsGranted: Boolean,
    accessibilityEnabled: Boolean,
    captureReady: Boolean,
    onNotifications: () -> Unit,
    onAccessibility: () -> Unit,
    onCapture: () -> Unit,
    onDone: () -> Unit,
    computerName: String = "",
    deviceLabel: String = "",
    onComputerName: (String) -> Unit = {},
) {
    var name by rememberSaveable { mutableStateOf(computerName) }
    PjPage {
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Allow PJDesk to host this phone", style = MaterialTheme.typography.titleLarge)
        Text(
            "Three permissions. Each one is asked by Android itself. You can control other computers without all three, but this phone cannot be controlled until they are granted.",
            color = pjMuted(),
        )
        OutlinedTextField(
            value = name,
            onValueChange = { name = it.take(64) },
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            label = { Text("Name this phone") },
            supportingText = {
                val shown = deviceLabel.ifBlank { "this phone" }
                Text("Optional. Computer name: $shown. Leave blank to use that.")
            },
        )
        Step(
            index = "1",
            title = "Notifications",
            body = "PJDesk keeps a notification while it is online. When someone is controlling this phone the notification says so, and Stop ends it.",
            done = notificationsGranted,
            action = "Allow notifications",
            onAction = onNotifications,
        )
        Step(
            index = "2",
            title = "Accessibility",
            body = "Remote taps, swipes, pinch, Back, Home, Recents, and typed text are delivered through an accessibility service. PJDesk does not read the screen for itself. Android opens the system setting; turn PJDesk on there.",
            done = accessibilityEnabled,
            action = "Open accessibility settings",
            onAction = onAccessibility,
        )
        Step(
            index = "3",
            title = "Screen sharing",
            body = "Android shows a system consent. On Android 14 and later that consent is used once, so PJDesk asks again when someone connects. This step only checks that the prompt appears.",
            done = captureReady,
            action = "Share the screen",
            onAction = onCapture,
        )
        Button(
            onClick = {
                onComputerName(name.trim())
                onDone()
            },
            enabled = notificationsGranted && accessibilityEnabled && captureReady,
            modifier = Modifier.fillMaxWidth(),
        ) { Text("Continue") }
    }
    }
}

@Composable
private fun Step(
    index: String,
    title: String,
    body: String,
    done: Boolean,
    action: String,
    onAction: () -> Unit,
) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("$index  $title", style = MaterialTheme.typography.titleMedium)
            Text(body, color = pjMuted())
            Text(if (done) "Granted" else "Not granted yet", color = if (done) MaterialTheme.colorScheme.primary else pjMuted())
            if (!done) Button(onClick = onAction) { Text(action) }
        }
    }
}
