package us.spader.pjdesk.ui

import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import us.spader.pjdesk.core.SessionInfo
import us.spader.pjdesk.input.GestureMap
import us.spader.pjdesk.input.PointerSession
import us.spader.pjdesk.media.ScreenDecoder

@Composable
fun SessionScreen(
    session: SessionInfo,
    onGesture: (GestureMap.Gesture) -> Unit,
    onMouse: (String, Int, Int, Int) -> Unit,
    onText: (String) -> Unit,
    onSpecial: (Int) -> Unit,
    onKey: (Int, Boolean) -> Unit,
    onQuality: (String) -> Unit,
    onDisplay: (Int) -> Unit,
    onEnd: () -> Unit,
    decoder: ScreenDecoder?,
) {
    val android = session.platform == "android"
    val pointer = remember(android) { PointerSession(android) }
    var viewW by remember { mutableIntStateOf(1) }
    var viewH by remember { mutableIntStateOf(1) }
    var cursor by remember { mutableStateOf(pointer.cursorNorm) }
    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black)
            .onSizeChanged {
                viewW = it.width.coerceAtLeast(1)
                viewH = it.height.coerceAtLeast(1)
            },
    ) {
        if (decoder != null) {
            AndroidView(
                factory = { context ->
                    SurfaceView(context).apply {
                        holder.addCallback(object : SurfaceHolder.Callback {
                            override fun surfaceCreated(holder: SurfaceHolder) {
                                decoder.setSurface(holder.surface)
                            }
                            override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
                                decoder.setSurface(holder.surface)
                            }
                            override fun surfaceDestroyed(holder: SurfaceHolder) {
                                decoder.setSurface(null)
                            }
                        })
                    }
                },
                modifier = Modifier.fillMaxSize(),
            )
        }
        Box(
            Modifier
                .fillMaxSize()
                .pointerInput(android, session.frameW, session.frameH, viewW, viewH) {
                val placed = if (android) {
                    GestureMap.letterbox(session.frameW, session.frameH, viewW, viewH)
                } else {
                    GestureMap.letterbox(viewW.coerceAtLeast(2), viewH.coerceAtLeast(2), viewW.coerceAtLeast(2), viewH.coerceAtLeast(2))
                }
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    fun points(): List<Pair<Int, Int>> =
                        currentEvent.changes.mapNotNull { change ->
                            GestureMap.viewToNorm(placed, change.position.x, change.position.y)
                        }
                    pointer.down(down.uptimeMillis, points())
                    while (true) {
                        val event = awaitPointerEvent()
                        val pressed = event.changes.any { it.pressed }
                        if (!pressed) {
                            pointer.up(event.changes.first().uptimeMillis).forEach { out ->
                                dispatch(out, onGesture, onMouse)
                            }
                            cursor = pointer.cursorNorm
                            break
                        }
                        pointer.move(points()).forEach { out -> dispatch(out, onGesture, onMouse) }
                        pointer.poll(event.changes.first().uptimeMillis)?.let { dispatch(it, onGesture, onMouse) }
                        cursor = pointer.cursorNorm
                        event.changes.forEach { it.consume() }
                    }
                }
            },
        ) {
            if (!android) {
                val placed = GestureMap.letterbox(
                    if (session.frameW > 0) session.frameW else 16,
                    if (session.frameH > 0) session.frameH else 9,
                    viewW,
                    viewH,
                )
                val px = if (placed.width > 1) {
                    placed.x + (cursor.first / GestureMap.NORM.toFloat()) * (placed.width - 1)
                } else {
                    viewW / 2f
                }
                val py = if (placed.height > 1) {
                    placed.y + (cursor.second / GestureMap.NORM.toFloat()) * (placed.height - 1)
                } else {
                    viewH / 2f
                }
                CursorMark(px, py)
            }
        }
        SessionChrome(
            session = session,
            modifier = Modifier.align(Alignment.BottomCenter),
            onText = onText,
            onSpecial = onSpecial,
            onKey = onKey,
            onQuality = onQuality,
            onDisplay = onDisplay,
            onEnd = onEnd,
        )
    }
}

@Composable
private fun CursorMark(x: Float, y: Float) {
    androidx.compose.foundation.Canvas(Modifier.fillMaxSize()) {
        drawCircle(Color.White, radius = 6f, center = androidx.compose.ui.geometry.Offset(x, y))
        drawCircle(Color.Black, radius = 3f, center = androidx.compose.ui.geometry.Offset(x, y))
    }
}

private fun dispatch(
    out: PointerSession.Out,
    onGesture: (GestureMap.Gesture) -> Unit,
    onMouse: (String, Int, Int, Int) -> Unit,
) {
    when (out) {
        is PointerSession.Out.Gesture -> onGesture(out.gesture)
        is PointerSession.Out.Mouse -> onMouse(out.kind, out.x, out.y, out.dy)
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun SessionChrome(
    session: SessionInfo,
    modifier: Modifier = Modifier,
    onText: (String) -> Unit,
    onSpecial: (Int) -> Unit,
    onKey: (Int, Boolean) -> Unit,
    onQuality: (String) -> Unit,
    onDisplay: (Int) -> Unit,
    onEnd: () -> Unit,
) {
    val android = session.platform == "android"
    var keyboard by remember { mutableStateOf(false) }
    var typed by remember { mutableStateOf("") }
    var keys by remember { mutableStateOf(false) }
    var quality by remember { mutableStateOf(false) }
    var displays by remember { mutableStateOf(false) }
    Surface(
        modifier = modifier.fillMaxWidth().padding(8.dp),
        shape = RoundedCornerShape(12.dp),
        color = MaterialTheme.colorScheme.surface.copy(alpha = 0.94f),
        tonalElevation = 4.dp,
    ) {
        Column(Modifier.padding(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(
                "${session.peer.ifBlank { session.id }} · ${osLabel(session.platform).ifBlank { "Remote" }} · ${session.path}",
                style = MaterialTheme.typography.labelMedium,
                color = pjMuted(),
            )
            FlowRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                TextButton(onClick = { keyboard = !keyboard }) { Text("Keyboard") }
                if (android) {
                    TextButton(onClick = { onSpecial(3) }) { Text("Back") }
                    TextButton(onClick = { onSpecial(4) }) { Text("Home") }
                    TextButton(onClick = { onSpecial(5) }) { Text("Recents") }
                } else {
                    TextButton(onClick = { onSpecial(0) }) { Text("Ctrl+Alt+Del") }
                    TextButton(onClick = { keys = true }) { Text("Keys") }
                    DropdownMenu(expanded = keys, onDismissRequest = { keys = false }) {
                        WinKeys.forEach { (label, scan) ->
                            DropdownMenuItem(
                                text = { Text(label) },
                                onClick = {
                                    keys = false
                                    onKey(scan, true)
                                    onKey(scan, false)
                                },
                            )
                        }
                    }
                }
                TextButton(onClick = { quality = true }) { Text("Quality") }
                DropdownMenu(expanded = quality, onDismissRequest = { quality = false }) {
                    listOf("smooth", "balanced", "sharp", "high").forEach { name ->
                        DropdownMenuItem(text = { Text(name) }, onClick = { quality = false; onQuality(name) })
                    }
                }
                if (session.displays.size > 1) {
                    TextButton(onClick = { displays = true }) { Text("Display") }
                    DropdownMenu(expanded = displays, onDismissRequest = { displays = false }) {
                        session.displays.forEach { display ->
                            DropdownMenuItem(
                                text = { Text(display.name.ifBlank { "Display ${display.index}" }) },
                                onClick = { displays = false; onDisplay(display.index) },
                            )
                        }
                    }
                }
                TextButton(onClick = onEnd) { Text("End") }
            }
            if (keyboard) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(
                        value = typed,
                        onValueChange = { next ->
                            val added = if (next.length >= typed.length && next.startsWith(typed)) {
                                next.removePrefix(typed)
                            } else {
                                "\u0008".repeat((typed.length - next.length).coerceAtLeast(1))
                            }
                            if (added.isNotEmpty()) onText(added)
                            typed = next
                        },
                        modifier = Modifier.weight(1f),
                        singleLine = true,
                        label = { Text("Type") },
                    )
                    Button(onClick = { onText("\n"); typed = "" }) { Text("Enter") }
                }
            }
        }
    }
}

/** PC set-1 scan codes, matching the Windows client's keymap. */
private val WinKeys = listOf(
    "Esc" to 0x01,
    "Tab" to 0x0F,
    "Win" to 0xE05B,
    "Alt" to 0x38,
    "Ctrl" to 0x1D,
    "Shift" to 0x2A,
    "↑" to 0xE048,
    "↓" to 0xE050,
    "←" to 0xE04B,
    "→" to 0xE04D,
    "Del" to 0xE053,
)
