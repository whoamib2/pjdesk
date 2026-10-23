package us.spader.pjdesk.ui

import android.os.Build
import android.view.HapticFeedbackConstants
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.R
import us.spader.pjdesk.core.AppLockPolicy

@Composable
fun SetPinScreen(
    email: String,
    onEmail: (String) -> Unit,
    needPassword: Boolean,
    busy: Boolean,
    error: String,
    biometricsAvailable: Boolean,
    biometricsDefault: Boolean = biometricsAvailable,
    title: String,
    detail: String,
    onSave: (pin: String, password: String, enableBio: Boolean) -> Unit,
    onCancel: (() -> Unit)?,
) {
    var pin by remember { mutableStateOf("") }
    var confirm by remember { mutableStateOf("") }
    var confirming by remember { mutableStateOf(false) }
    var password by remember { mutableStateOf("") }
    var enableBio by remember { mutableStateOf(biometricsDefault) }
    var localError by remember { mutableStateOf("") }
    var shakeToken by remember { mutableIntStateOf(0) }
    val entered = if (confirming) confirm else pin
    val shown = localError.ifBlank { error }
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp, vertical = 32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        LockMark()
        Text(title, style = MaterialTheme.typography.headlineMedium, textAlign = TextAlign.Center)
        Text(detail, color = pjMuted(), textAlign = TextAlign.Center, style = MaterialTheme.typography.bodyMedium)
        if (needPassword) {
            AccountEmailField(value = email, onValueChange = onEmail, label = "Email", modifier = Modifier.fillMaxWidth())
            AccountPasswordField(
                value = password,
                onValueChange = { password = it },
                label = "Account password",
                modifier = Modifier.fillMaxWidth(),
            )
            Text(
                "This is your pjdesk.com password, used once to unlock saved computer passwords.",
                color = pjMuted(),
                style = MaterialTheme.typography.bodySmall,
            )
        }
        Text(
            if (confirming) "Confirm PIN" else "Choose a PIN",
            style = MaterialTheme.typography.titleLarge,
        )
        PinDots(count = entered.length, shakeToken = shakeToken)
        if (shown.isNotEmpty()) {
            Text(shown, color = MaterialTheme.colorScheme.error, textAlign = TextAlign.Center)
        }
        PinKeypad(
            enabled = !busy,
            showBiometric = false,
            onDigit = { digit ->
                localError = ""
                if (confirming) {
                    if (confirm.length < 8) confirm += digit
                } else if (pin.length < 8) {
                    pin += digit
                }
            },
            onBackspace = {
                localError = ""
                if (confirming) {
                    if (confirm.isNotEmpty()) confirm = confirm.dropLast(1)
                } else if (pin.isNotEmpty()) {
                    pin = pin.dropLast(1)
                }
            },
            onBiometric = {},
        )
        if (biometricsAvailable) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Checkbox(checked = enableBio, onCheckedChange = { enableBio = it }, enabled = !busy)
                Text("Unlock with fingerprint or face")
            }
        }
        Button(
            onClick = {
                localError = when {
                    needPassword && password.isBlank() -> "Enter your account password."
                    !confirming && !AppLockPolicy.pinAcceptable(pin) -> "Use 4 to 8 digits."
                    confirming && confirm != pin -> "Those PINs do not match."
                    confirming && !AppLockPolicy.pinAcceptable(confirm) -> "Use 4 to 8 digits."
                    else -> ""
                }
                if (localError == "Those PINs do not match.") {
                    shakeToken++
                    confirm = ""
                } else if (localError.isEmpty() && !confirming) {
                    confirming = true
                } else if (localError.isEmpty()) {
                    onSave(pin, password, enableBio && biometricsAvailable)
                }
            },
            enabled = !busy && AppLockPolicy.pinAcceptable(entered),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text(
                when {
                    busy -> "Saving…"
                    confirming -> "Save PIN"
                    else -> "Continue"
                },
            )
        }
        if (confirming) {
            TextButton(onClick = { confirming = false; confirm = ""; localError = "" }, enabled = !busy) {
                Text("Start over")
            }
        }
        if (onCancel != null) TextButton(onClick = onCancel, enabled = !busy) { Text("Cancel") }
    }
}

@Composable
fun UnlockScreen(
    busy: Boolean,
    error: String,
    biometrics: Boolean,
    rejectCount: Int = 0,
    onSubmit: (String) -> Unit,
    onBiometric: () -> Unit,
) {
    var pin by remember { mutableStateOf("") }
    LaunchedEffect(rejectCount) {
        if (rejectCount > 0) pin = ""
    }
    Column(
        Modifier.fillMaxSize().padding(horizontal = 24.dp, vertical = 32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        LockMark()
        Spacer(Modifier.size(16.dp))
        Text("Enter PIN", style = MaterialTheme.typography.headlineMedium)
        Spacer(Modifier.size(8.dp))
        Text(
            "Your account stays signed in.",
            color = pjMuted(),
            style = MaterialTheme.typography.bodyMedium,
        )
        Spacer(Modifier.size(24.dp))
        PinDots(count = pin.length, shakeToken = rejectCount)
        if (error.isNotEmpty()) {
            Spacer(Modifier.size(12.dp))
            Text(error, color = MaterialTheme.colorScheme.error, textAlign = TextAlign.Center)
        }
        Spacer(Modifier.size(28.dp))
        PinKeypad(
            enabled = !busy,
            showBiometric = biometrics,
            onDigit = { digit ->
                if (pin.length >= 8) return@PinKeypad
                val next = pin + digit
                pin = next
                if (next.length == 8) onSubmit(next)
            },
            onBackspace = { if (pin.isNotEmpty()) pin = pin.dropLast(1) },
            onBiometric = onBiometric,
        )
        Spacer(Modifier.size(20.dp))
        Button(
            onClick = { if (AppLockPolicy.pinAcceptable(pin)) onSubmit(pin) },
            enabled = !busy && AppLockPolicy.pinAcceptable(pin),
            modifier = Modifier.fillMaxWidth(),
        ) { Text(if (busy) "Checking…" else "Unlock") }
    }
}

@Composable
fun ReauthScreen(
    email: String,
    onEmail: (String) -> Unit,
    busy: Boolean,
    error: String,
    onSubmit: (String) -> Unit,
) {
    var password by remember { mutableStateOf("") }
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        LockMark()
        Text("Sign in with your password", style = MaterialTheme.typography.headlineMedium)
        Text(
            "The PIN was not accepted too many times, or saved passwords need your account password again. You will set a new PIN after this.",
            color = pjMuted(),
        )
        AccountEmailField(value = email, onValueChange = onEmail, label = "Email")
        AccountPasswordField(value = password, onValueChange = { password = it }, label = "Account password")
        if (error.isNotEmpty()) Text(error, color = MaterialTheme.colorScheme.error)
        Button(
            onClick = { if (password.isNotBlank()) onSubmit(password) },
            enabled = !busy && password.isNotBlank(),
            modifier = Modifier.fillMaxWidth(),
        ) { Text(if (busy) "Signing in…" else "Sign in with your password") }
    }
}

@Composable
private fun LockMark() {
    Image(
        painter = painterResource(R.drawable.ic_launcher),
        contentDescription = "PJDesk",
        modifier = Modifier.size(72.dp).clip(RoundedCornerShape(18.dp)),
    )
}

@Composable
private fun PinDots(count: Int, shakeToken: Int) {
    val shift = shakeOffset(shakeToken)
    Row(
        Modifier.offset(x = shift.dp),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        repeat(8) { index ->
            val filled = index < count
            Box(
                Modifier
                    .size(if (filled) 12.dp else 10.dp)
                    .clip(CircleShape)
                    .background(if (filled) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline),
            )
        }
    }
}

@Composable
private fun shakeOffset(token: Int): Float {
    val view = LocalView.current
    val offset = remember { Animatable(0f) }
    LaunchedEffect(token) {
        if (token == 0) return@LaunchedEffect
        val constant = if (Build.VERSION.SDK_INT >= 30) {
            HapticFeedbackConstants.REJECT
        } else {
            HapticFeedbackConstants.LONG_PRESS
        }
        view.performHapticFeedback(constant)
        offset.snapTo(0f)
        listOf(16f, -14f, 10f, -6f, 0f).forEach { target ->
            offset.animateTo(target, tween(40))
        }
    }
    return offset.value
}

@Composable
private fun PinKeypad(
    enabled: Boolean,
    showBiometric: Boolean,
    onDigit: (Char) -> Unit,
    onBackspace: () -> Unit,
    onBiometric: () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        listOf("123", "456", "789").forEach { row ->
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                row.forEach { digit ->
                    PinKey(enabled = enabled, onClick = { onDigit(digit) }) {
                        Text(digit.toString(), style = MaterialTheme.typography.headlineMedium)
                    }
                }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            if (showBiometric) {
                PinKey(enabled = enabled, onClick = onBiometric) { FingerprintMark() }
            } else {
                Spacer(Modifier.size(76.dp))
            }
            PinKey(enabled = enabled, onClick = { onDigit('0') }) {
                Text("0", style = MaterialTheme.typography.headlineMedium)
            }
            PinKey(enabled = enabled, onClick = onBackspace) {
                Text("⌫", style = MaterialTheme.typography.headlineMedium)
            }
        }
    }
}

@Composable
private fun PinKey(enabled: Boolean, onClick: () -> Unit, content: @Composable () -> Unit) {
    Surface(
        onClick = onClick,
        enabled = enabled,
        shape = CircleShape,
        color = MaterialTheme.colorScheme.surfaceVariant,
        modifier = Modifier.size(76.dp),
    ) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { content() }
    }
}

@Composable
private fun FingerprintMark() {
    val color = MaterialTheme.colorScheme.onSurface
    Canvas(Modifier.size(28.dp)) {
        val stroke = Stroke(width = size.minDimension * 0.08f, cap = StrokeCap.Round)
        val cx = size.width / 2f
        val cy = size.height / 2f
        drawArc(
            color,
            startAngle = 200f,
            sweepAngle = 140f,
            useCenter = false,
            topLeft = Offset(cx - size.width * 0.16f, cy - size.height * 0.22f),
            size = Size(size.width * 0.32f, size.height * 0.46f),
            style = stroke,
        )
        drawArc(
            color,
            startAngle = 200f,
            sweepAngle = 140f,
            useCenter = false,
            topLeft = Offset(cx - size.width * 0.30f, cy - size.height * 0.34f),
            size = Size(size.width * 0.60f, size.height * 0.70f),
            style = stroke,
        )
        drawLine(
            color,
            Offset(cx, cy),
            Offset(cx, cy + size.height * 0.28f),
            strokeWidth = stroke.width,
            cap = StrokeCap.Round,
        )
    }
}
