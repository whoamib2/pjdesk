package us.spader.pjdesk.ui

import android.app.Activity
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import kotlin.math.hypot
import kotlin.math.roundToInt
import us.spader.pjdesk.core.SessionInfo
import us.spader.pjdesk.input.GestureMap
import us.spader.pjdesk.input.PictureView
import us.spader.pjdesk.input.PointerSession
import us.spader.pjdesk.media.ScreenDecoder

@Composable
fun SessionScreen(
    session: SessionInfo,
    onGesture: (GestureMap.Gesture) -> Unit,
    onMouse: (String, Int, Int, Int, String) -> Unit,
    onText: (String) -> Unit,
    onSpecial: (Int) -> Unit,
    onKey: (Int, Boolean) -> Unit,
    onQuality: (String) -> Unit,
    onDisplay: (Int) -> Unit,
    onEnd: () -> Unit,
    onClipboard: () -> Unit = {},
    decoder: ScreenDecoder?,
) {
    val android = session.platform == "android"
    var trackpad by remember(session.id) { mutableStateOf(!android) }
    val pointer = remember(trackpad, android) { PointerSession(android, trackpad = trackpad) }
    var viewW by remember { mutableIntStateOf(1) }
    var viewH by remember { mutableIntStateOf(1) }
    var picture by remember(session.id) { mutableStateOf(PictureView.fit()) }
    var cursor by remember { mutableStateOf(pointer.cursorNorm) }
    var keyboard by remember { mutableStateOf(false) }
    val view = LocalView.current
    DisposableEffect(Unit) {
        val window = (view.context as? Activity)?.window
        val controller = window?.let { WindowCompat.getInsetsController(it, view) }
        controller?.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        controller?.hide(WindowInsetsCompat.Type.systemBars())
        onDispose { controller?.show(WindowInsetsCompat.Type.systemBars()) }
    }
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
                modifier = Modifier.layout { measurable, constraints ->
                    val placed = GestureMap.letterbox(
                        session.frameW.coerceAtLeast(16),
                        session.frameH.coerceAtLeast(9),
                        constraints.maxWidth,
                        constraints.maxHeight,
                    )
                    val placeable = measurable.measure(
                        androidx.compose.ui.unit.Constraints.fixed(
                            placed.width.coerceAtLeast(1),
                            placed.height.coerceAtLeast(1),
                        ),
                    )
                    layout(constraints.maxWidth, constraints.maxHeight) {
                        placeable.place(placed.x, placed.y)
                    }
                },
                update = { surface ->
                    surface.pivotX = surface.width / 2f
                    surface.pivotY = surface.height / 2f
                    surface.scaleX = picture.scale
                    surface.scaleY = picture.scale
                    surface.translationX = picture.panX
                    surface.translationY = picture.panY
                },
            )
        }
        Box(
            Modifier
                .fillMaxSize()
                .pointerInput(trackpad, android, session.frameW, session.frameH, viewW, viewH) {
                    var lastTap = 0L
                    awaitEachGesture {
                        val first = awaitFirstDown(requireUnconsumed = false)
                        val now = first.uptimeMillis
                        val doubleTap = !trackpad && now - lastTap in 1..280
                        if (doubleTap) {
                            val placed = GestureMap.letterbox(
                                session.frameW.coerceAtLeast(16),
                                session.frameH.coerceAtLeast(9),
                                viewW,
                                viewH,
                            )
                            picture = if (picture.scale <= 1.05f) {
                                PictureView.Frame(PictureView.oneToOne(session.frameW, placed.width), 0f, 0f)
                            } else {
                                PictureView.fit()
                            }
                            lastTap = 0L
                        }
                        var sawDown = false
                        var zooming = false
                        var scrolling = false
                        var fingers = 1
                        var startSpan = 0f
                        var startPicture = picture
                        var lastCentroid = Offset.Zero
                        var moved = 0f
                        fun pointOf(at: Offset): Pair<Int, Int>? {
                            return if (trackpad) {
                                GestureMap.viewToNorm(GestureMap.letterbox(viewW, viewH, viewW, viewH), at.x, at.y)
                            } else {
                                PictureView.screenToNorm(
                                    viewW, viewH, session.frameW, session.frameH, picture, at.x, at.y,
                                )
                            }
                        }
                        if (!doubleTap) {
                            pointOf(first.position)?.let {
                                pointer.down(first.uptimeMillis, listOf(it))
                                sawDown = true
                                cursor = if (trackpad) pointer.cursorNorm else it
                            }
                        }
                        while (true) {
                            val event = awaitPointerEvent()
                            val pressed = event.changes.filter { it.pressed }
                            if (pressed.size > fingers) fingers = pressed.size
                            if (pressed.size >= 3 && trackpad) {
                                if (sawDown) {
                                    pointer.cancel()
                                    sawDown = false
                                }
                                event.changes.forEach { it.consume() }
                            } else if (pressed.size >= 2) {
                                val a = pressed[0].position
                                val b = pressed[1].position
                                val span = hypot((a.x - b.x).toDouble(), (a.y - b.y).toDouble()).toFloat().coerceAtLeast(1f)
                                val centroid = Offset((a.x + b.x) / 2f, (a.y + b.y) / 2f)
                                if (startSpan == 0f) {
                                    startSpan = span
                                    startPicture = picture
                                    lastCentroid = centroid
                                    if (sawDown) {
                                        pointer.cancel()
                                        sawDown = false
                                    }
                                } else if (trackpad && kotlin.math.abs(span / startSpan - 1f) < 0.08f) {
                                    scrolling = true
                                    val p1 = pointOf(a)
                                    val p2 = pointOf(b)
                                    if (p1 != null && p2 != null) {
                                        if (!sawDown) {
                                            pointer.down(pressed[0].uptimeMillis, listOf(p1, p2))
                                            sawDown = true
                                        } else {
                                            pointer.move(listOf(p1, p2)).forEach { dispatch(it, onGesture, onMouse) }
                                        }
                                    }
                                } else {
                                    zooming = true
                                    picture = PictureView.zoomAbout(
                                        viewW, viewH, session.frameW, session.frameH,
                                        startPicture, centroid.x, centroid.y,
                                        startPicture.scale * span / startSpan,
                                    )
                                    moved += hypot(
                                        (centroid.x - lastCentroid.x).toDouble(),
                                        (centroid.y - lastCentroid.y).toDouble(),
                                    ).toFloat()
                                    lastCentroid = centroid
                                }
                                event.changes.forEach { if (it.positionChanged()) it.consume() }
                            } else if (pressed.size == 1 && !zooming && !scrolling && !doubleTap) {
                                val at = pressed[0].position
                                val point = pointOf(at)
                                if (point != null) {
                                    if (!sawDown) {
                                        pointer.down(pressed[0].uptimeMillis, listOf(point))
                                        sawDown = true
                                    } else {
                                        pointer.move(listOf(point)).forEach { dispatch(it, onGesture, onMouse) }
                                        pointer.poll(pressed[0].uptimeMillis)?.let { dispatch(it, onGesture, onMouse) }
                                    }
                                    cursor = if (trackpad) pointer.cursorNorm else point
                                    if (trackpad && picture.scale > 1.05f) {
                                        picture = PictureView.followCursor(
                                            viewW, viewH, session.frameW, session.frameH, picture, cursor,
                                        )
                                    }
                                }
                                event.changes.forEach { it.consume() }
                            }
                            if (pressed.isEmpty()) {
                                if (fingers >= 3 && trackpad) {
                                    onMouse("down", cursor.first, cursor.second, 0, "middle")
                                    onMouse("up", cursor.first, cursor.second, 0, "middle")
                                } else if (startSpan > 0f && moved < 36f && !trackpad && !scrolling) {
                                    val point = PictureView.screenToNorm(
                                        viewW, viewH, session.frameW, session.frameH, picture,
                                        lastCentroid.x, lastCentroid.y,
                                    )
                                    if (point != null) {
                                        onMouse("down", point.first, point.second, 0, "right")
                                        onMouse("up", point.first, point.second, 0, "right")
                                    }
                                } else if ((scrolling || !zooming) && !doubleTap && sawDown) {
                                    pointer.up(event.changes.firstOrNull()?.uptimeMillis ?: now).forEach {
                                        dispatch(it, onGesture, onMouse)
                                    }
                                    cursor = if (trackpad) pointer.cursorNorm else cursor
                                    lastTap = event.changes.firstOrNull()?.uptimeMillis ?: now
                                }
                                pointer.cancel()
                                break
                            }
                        }
                    }
                },
        ) {
            val placed = GestureMap.letterbox(
                session.frameW.coerceAtLeast(16),
                session.frameH.coerceAtLeast(9),
                viewW,
                viewH,
            )
            val dw = placed.width * picture.scale
            val dh = placed.height * picture.scale
            val left = viewW / 2f + picture.panX - dw / 2f
            val top = viewH / 2f + picture.panY - dh / 2f
            val px = left + (cursor.first / GestureMap.NORM.toFloat()) * dw
            val py = top + (cursor.second / GestureMap.NORM.toFloat()) * dh
            CursorMark(px, py)
        }
        if (keyboard) {
            ModifierBar(
                modifier = Modifier.align(Alignment.BottomCenter),
                onText = onText,
                onKey = onKey,
                onClose = { keyboard = false },
            )
        }
        SessionChrome(
            session = session,
            modifier = Modifier.align(Alignment.TopStart),
            trackpad = trackpad,
            fitted = picture.scale <= 1.05f,
            onTrackpad = { trackpad = it },
            onFit = { fit ->
                picture = if (fit) {
                    PictureView.fit()
                } else {
                    val placed = GestureMap.letterbox(
                        session.frameW.coerceAtLeast(16),
                        session.frameH.coerceAtLeast(9),
                        viewW,
                        viewH,
                    )
                    PictureView.Frame(PictureView.oneToOne(session.frameW, placed.width), 0f, 0f)
                }
            },
            onText = { _ -> keyboard = !keyboard },
            onSpecial = onSpecial,
            onKey = onKey,
            onQuality = onQuality,
            onDisplay = onDisplay,
            onClipboard = onClipboard,
            onEnd = onEnd,
        )
    }
}

@Composable
private fun CursorMark(x: Float, y: Float) {
    Canvas(Modifier.fillMaxSize()) {
        drawCircle(Color.White, radius = 7f, center = Offset(x, y))
        drawCircle(Color.Black, radius = 3.5f, center = Offset(x, y))
    }
}

private fun dispatch(
    out: PointerSession.Out,
    onGesture: (GestureMap.Gesture) -> Unit,
    onMouse: (String, Int, Int, Int, String) -> Unit,
) {
    when (out) {
        is PointerSession.Out.Gesture -> onGesture(out.gesture)
        is PointerSession.Out.Mouse -> onMouse(out.kind, out.x, out.y, out.dy, out.button)
    }
}

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
    onClipboard: () -> Unit = {},
    trackpad: Boolean = false,
    fitted: Boolean = true,
    onTrackpad: (Boolean) -> Unit = {},
    onFit: (Boolean) -> Unit = {},
) {
    var open by remember { mutableStateOf(false) }
    var ox by remember { mutableFloatStateOf(16f) }
    var oy by remember { mutableFloatStateOf(24f) }
    var quality by remember { mutableStateOf(false) }
    var displays by remember { mutableStateOf(false) }
    var dragged by remember { mutableFloatStateOf(0f) }
    Surface(
        modifier = modifier
            .offset { IntOffset(ox.roundToInt(), oy.roundToInt()) }
            .pointerInput(Unit) {
                detectDragGestures(
                    onDragStart = { dragged = 0f },
                    onDragEnd = {
                        if (dragged < 12f) open = !open
                    },
                ) { change, delta ->
                    change.consume()
                    dragged += hypot(delta.x.toDouble(), delta.y.toDouble()).toFloat()
                    ox += delta.x
                    oy += delta.y
                }
            },
        shape = if (open) RoundedCornerShape(20.dp) else CircleShape,
        color = MaterialTheme.colorScheme.surface.copy(alpha = 0.94f),
        tonalElevation = 6.dp,
    ) {
        if (!open) {
            Box(Modifier.size(56.dp), contentAlignment = Alignment.Center) {
                Mark("tools")
            }
        } else {
            Column(Modifier.padding(6.dp).widthIn(max = 280.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    session.peer.ifBlank { session.id }.take(28),
                    style = MaterialTheme.typography.labelMedium,
                    color = pjMuted(),
                    modifier = Modifier.padding(horizontal = 8.dp),
                )
                Row {
                    Tool("keyboard", "Keyboard") { onText("") }
                    Tool("keys", "Ctrl+Alt+Del") { onSpecial(0) }
                    Tool(if (trackpad) "touch" else "pad", if (trackpad) "Touch" else "Trackpad") {
                        onTrackpad(!trackpad)
                    }
                    Tool(if (fitted) "actual" else "fit", if (fitted) "1:1" else "Fit") { onFit(!fitted) }
                }
                Row {
                    Box {
                        Tool("display", "Display") { displays = true }
                        DropdownMenu(expanded = displays, onDismissRequest = { displays = false }) {
                            val list = session.displays.ifEmpty {
                                listOf(us.spader.pjdesk.core.DisplayInfo(0, "This display", session.frameW, session.frameH, true))
                            }
                            list.forEach { display ->
                                DropdownMenuItem(
                                    text = { Text(display.name.ifBlank { "Display ${display.index}" }) },
                                    onClick = { displays = false; onDisplay(display.index) },
                                )
                            }
                        }
                    }
                    Box {
                        Tool("quality", "Quality") { quality = true }
                        DropdownMenu(expanded = quality, onDismissRequest = { quality = false }) {
                            listOf("smooth", "balanced", "sharp", "high").forEach { name ->
                                DropdownMenuItem(
                                    text = { Text(name) },
                                    onClick = { quality = false; onQuality(name) },
                                )
                            }
                        }
                    }
                    Tool("clip", "Clipboard") { onClipboard() }
                    Tool("end", "End") { onEnd() }
                }
            }
        }
    }
}

@Composable
private fun Tool(kind: String, label: String, onClick: () -> Unit) {
    IconButton(onClick = onClick) { Mark(kind) }
}

@Composable
private fun ModifierBar(
    modifier: Modifier,
    onText: (String) -> Unit,
    onKey: (Int, Boolean) -> Unit,
    onClose: () -> Unit,
) {
    var typed by remember { mutableStateOf("") }
    var held by remember { mutableStateOf(setOf<Int>()) }
    Surface(
        modifier = modifier.padding(8.dp),
        shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.surface.copy(alpha = 0.96f),
    ) {
        Column(Modifier.padding(8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(
                Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                listOf("Ctrl" to 0x1D, "Alt" to 0x38, "Win" to 0xE05B, "Shift" to 0x2A).forEach { (label, scan) ->
                    FilterChip(
                        selected = held.contains(scan),
                        onClick = {
                            if (held.contains(scan)) {
                                held = held - scan
                                onKey(scan, false)
                            } else {
                                held = held + scan
                                onKey(scan, true)
                            }
                        },
                        label = { Text(label) },
                    )
                }
                WinKeys.forEach { (label, scan) ->
                    FilterChip(
                        selected = false,
                        onClick = {
                            onKey(scan, true)
                            onKey(scan, false)
                        },
                        label = { Text(label) },
                    )
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
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
                IconButton(onClick = { onText("\n"); typed = "" }) { Mark("enter") }
                IconButton(onClick = onClose) { Mark("end") }
            }
        }
    }
}

@Composable
private fun Mark(kind: String) {
    val color = MaterialTheme.colorScheme.onSurface
    Canvas(Modifier.size(22.dp)) {
        val stroke = Stroke(width = 1.7.dp.toPx())
        when (kind) {
            "keyboard", "keys" -> {
                drawRoundRect(color, topLeft = Offset(2f, size.height * 0.28f), size = Size(size.width - 4f, size.height * 0.5f), cornerRadius = CornerRadius(3f, 3f), style = stroke)
                drawLine(color, Offset(size.width * 0.22f, size.height * 0.62f), Offset(size.width * 0.78f, size.height * 0.62f), strokeWidth = 1.6.dp.toPx())
            }
            "touch" -> drawCircle(color, radius = size.minDimension * 0.28f, style = stroke)
            "pad" -> drawRoundRect(color, topLeft = Offset(size.width * 0.18f, size.height * 0.22f), size = Size(size.width * 0.64f, size.height * 0.56f), cornerRadius = CornerRadius(8f, 8f), style = stroke)
            "display" -> drawRoundRect(color, topLeft = Offset(3f, size.height * 0.2f), size = Size(size.width - 6f, size.height * 0.55f), cornerRadius = CornerRadius(2f, 2f), style = stroke)
            "quality" -> drawCircle(color, radius = size.minDimension * 0.22f, center = center, style = stroke)
            "clip" -> drawRoundRect(color, topLeft = Offset(size.width * 0.28f, size.height * 0.18f), size = Size(size.width * 0.44f, size.height * 0.62f), cornerRadius = CornerRadius(2f, 2f), style = stroke)
            "fit", "actual" -> {
                drawLine(color, Offset(4f, 4f), Offset(size.width * 0.42f, 4f), strokeWidth = 1.6.dp.toPx())
                drawLine(color, Offset(4f, 4f), Offset(4f, size.height * 0.42f), strokeWidth = 1.6.dp.toPx())
                drawLine(color, Offset(size.width - 4f, size.height - 4f), Offset(size.width * 0.58f, size.height - 4f), strokeWidth = 1.6.dp.toPx())
                drawLine(color, Offset(size.width - 4f, size.height - 4f), Offset(size.width - 4f, size.height * 0.58f), strokeWidth = 1.6.dp.toPx())
            }
            "tools" -> {
                drawCircle(color, radius = 2.2.dp.toPx(), center = Offset(size.width * 0.3f, size.height / 2f))
                drawCircle(color, radius = 2.2.dp.toPx(), center = center)
                drawCircle(color, radius = 2.2.dp.toPx(), center = Offset(size.width * 0.7f, size.height / 2f))
            }
            else -> drawLine(color, Offset(6f, 6f), Offset(size.width - 6f, size.height - 6f), strokeWidth = 1.8.dp.toPx())
        }
    }
}

/** PC set-1 scan codes, matching the Windows client's keymap. */
private val WinKeys = listOf(
    "Esc" to 0x01,
    "Tab" to 0x0F,
    "↑" to 0xE048,
    "↓" to 0xE050,
    "←" to 0xE04B,
    "→" to 0xE04D,
    "Del" to 0xE053,
    "F1" to 0x3B,
    "F2" to 0x3C,
    "F3" to 0x3D,
    "F4" to 0x3E,
    "F5" to 0x3F,
    "F6" to 0x40,
    "F7" to 0x41,
    "F8" to 0x42,
    "F9" to 0x43,
    "F10" to 0x44,
    "F11" to 0x57,
    "F12" to 0x58,
)
