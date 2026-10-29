@file:OptIn(
    androidx.compose.foundation.ExperimentalFoundationApi::class,
    androidx.compose.material3.ExperimentalMaterial3Api::class,
)

package us.spader.pjdesk.ui

import android.app.Activity
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.systemGestures
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.ripple
import androidx.compose.runtime.Composable
import kotlinx.coroutines.delay
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
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
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.zIndex
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import kotlin.math.hypot
import kotlin.math.roundToInt
import us.spader.pjdesk.core.ChatLine
import us.spader.pjdesk.core.ControlPad
import us.spader.pjdesk.core.RemoteOptions
import us.spader.pjdesk.core.SessionHit
import us.spader.pjdesk.core.SessionInfo
import us.spader.pjdesk.input.GestureMap
import us.spader.pjdesk.input.PictureView
import us.spader.pjdesk.input.PointerSession
import us.spader.pjdesk.media.ScreenDecoder

private class OverlaySpace {
    val pads = HashMap<String, ControlPad>()
    var origin = Offset.Zero
}

@Composable
fun SessionScreen(
    session: SessionInfo,
    onGesture: (GestureMap.Gesture) -> Unit,
    onMouse: (String, Int, Int, Int, String) -> Unit,
    onText: (String) -> Unit,
    onSpecial: (Int) -> Unit,
    onKey: (Int, Boolean) -> Unit,
    onQuality: (String) -> Unit,
    onQualityCustom: (Int, Int) -> Unit = { _, _ -> },
    onPrivacy: (Boolean) -> Unit = {},
    onKeyboardMode: (String) -> Unit = {},
    onDisplay: (Int) -> Unit,
    onEnd: () -> Unit,
    onClipboard: () -> Unit = {},
    decoder: ScreenDecoder?,
    remote: RemoteOptions = RemoteOptions(),
    onRemote: (RemoteOptions) -> Unit = {},
    forThisComputer: Boolean = false,
    onForThisComputer: (Boolean) -> Unit = {},
    fileNames: List<String> = emptyList(),
    onFileList: () -> Unit = {},
    onSendFile: () -> Unit = {},
    onRestart: () -> Unit = {},
    idleWarn: Boolean = false,
    onStay: () -> Unit = {},
    onActivity: () -> Unit = {},
    chatReady: Boolean = false,
    chatTooOld: Boolean = false,
    chatLines: List<ChatLine> = emptyList(),
    chatUnread: Int = 0,
    onChat: (String) -> Unit = {},
    onChatOpen: () -> Unit = {},
    reconnecting: Boolean = false,
    reconnectSeconds: Int = 0,
    onReconnectNow: () -> Unit = {},
) {
    val android = session.platform == "android"
    val trackpad = remote.trackpadFor(android)
    val pointer = remember(trackpad, android) { PointerSession(android, trackpad = trackpad) }
    val options = rememberUpdatedState(remote)
    val space = remember { OverlaySpace() }
    var confirmEnd by remember { mutableStateOf(false) }
    var sheet by remember { mutableStateOf(false) }
    var chatSheet by remember { mutableStateOf(false) }
    var shownDisplay by remember(session.id) {
        mutableIntStateOf(session.displays.firstOrNull { it.primary }?.index ?: 0)
    }
    val peerName = session.peer.ifBlank { session.id }.ifBlank { "this computer" }
    BackHandler {
        when {
            chatSheet -> chatSheet = false
            sheet -> sheet = false
            else -> confirmEnd = true
        }
    }
    LaunchedEffect(chatSheet, chatLines.size) {
        if (chatSheet) onChatOpen()
    }
    var viewW by remember { mutableIntStateOf(1) }
    var viewH by remember { mutableIntStateOf(1) }
    var picture by remember(session.id) { mutableStateOf(PictureView.fit()) }
    var cursor by remember { mutableStateOf(pointer.cursorNorm) }
    val view = LocalView.current
    DisposableEffect(Unit) {
        val window = (view.context as? Activity)?.window
        val controller = window?.let { WindowCompat.getInsetsController(it, view) }
        controller?.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        controller?.hide(WindowInsetsCompat.Type.systemBars())
        onDispose { controller?.show(WindowInsetsCompat.Type.systemBars()) }
    }
    var retryLeft by remember(reconnecting, reconnectSeconds) { mutableIntStateOf(reconnectSeconds) }
    LaunchedEffect(reconnecting, reconnectSeconds) {
        retryLeft = reconnectSeconds
        while (reconnecting && retryLeft > 0) {
            delay(1_000)
            retryLeft -= 1
        }
    }
    LaunchedEffect(session.id, remote.quality, remote.customFps, remote.customBitrate) {
        if (remote.quality == "custom") onQualityCustom(remote.customFps, remote.customBitrate)
        else onQuality(RemoteOptions.preset(remote.quality))
    }
    LaunchedEffect(session.id, remote.privacy) { onPrivacy(remote.privacy) }
    LaunchedEffect(session.id, remote.keyboardMode) { onKeyboardMode(remote.keyboardMode) }
    LaunchedEffect(remote.scale, session.frameW, viewW, viewH) {
        picture = when (remote.scale) {
            "original" -> {
                val placed = GestureMap.letterbox(
                    session.frameW.coerceAtLeast(16),
                    session.frameH.coerceAtLeast(9),
                    viewW,
                    viewH,
                )
                PictureView.Frame(PictureView.oneToOne(session.frameW, placed.width), 0f, 0f)
            }
            else -> PictureView.fit()
        }
    }
    val bars = WindowInsets.safeDrawing.union(WindowInsets.systemGestures).union(WindowInsets.displayCutout)
    Box(
        Modifier
            .fillMaxSize()
            .background(Color.Black)
            .onSizeChangedSafe {
                viewW = it.width.coerceAtLeast(1)
                viewH = it.height.coerceAtLeast(1)
            },
    ) {
        if (decoder != null) {
            AndroidView(
                factory = { context ->
                    SurfaceView(context).apply {
                        setZOrderOnTop(false)
                        setZOrderMediaOverlay(false)
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
                modifier = Modifier.zIndex(0f).layout { measurable, constraints ->
                    if (options.value.scale == "stretch") {
                        val placeable = measurable.measure(
                            androidx.compose.ui.unit.Constraints.fixed(
                                constraints.maxWidth.coerceAtLeast(1),
                                constraints.maxHeight.coerceAtLeast(1),
                            ),
                        )
                        layout(constraints.maxWidth, constraints.maxHeight) { placeable.place(0, 0) }
                    } else {
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
                    }
                },
                update = { surface ->
                    surface.pivotX = surface.width / 2f
                    surface.pivotY = surface.height / 2f
                    surface.scaleX = if (options.value.scale == "stretch") 1f else picture.scale
                    surface.scaleY = if (options.value.scale == "stretch") 1f else picture.scale
                    surface.translationX = if (options.value.scale == "stretch") 0f else picture.panX
                    surface.translationY = if (options.value.scale == "stretch") 0f else picture.panY
                },
            )
        }
        Box(
            Modifier
                .fillMaxSize()
                .zIndex(1f)
                .onGloballyPositioned { space.origin = it.positionInRoot() }
                .pointerInput(trackpad, android, session.frameW, session.frameH, viewW, viewH) {
                    var lastTap = 0L
                    awaitEachGesture {
                        val first = awaitFirstDown(requireUnconsumed = true)
                        if (SessionHit.covered(first.position.x, first.position.y, space.pads.values.toList())) {
                            return@awaitEachGesture
                        }
                        onActivity()
                        val current = options.value
                        pointer.speed = current.pointerSpeed
                        pointer.accelerate = current.pointerAccel
                        pointer.dragLock = current.dragLock
                        pointer.scrollSpeed = current.scrollSpeed
                        pointer.invertScroll = current.invertScroll
                        val viewOnly = current.viewOnly
                        val stretch = current.scale == "stretch"
                        fun send(out: PointerSession.Out) {
                            if (viewOnly) return
                            dispatch(out, onGesture, onMouse)
                        }
                        val now = first.uptimeMillis
                        val repeat = now - lastTap in 1..280
                        if (repeat && trackpad && !viewOnly) pointer.armDrag()
                        val doubleTap = repeat && !trackpad
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
                        var lastAt = first.position
                        var panning = false
                        var moved = 0f
                        fun dropPointer() {
                            pointer.cancel().forEach { send(it) }
                        }
                        fun pointOf(at: Offset): Pair<Int, Int>? {
                            if (stretch) {
                                if (viewW < 2 || viewH < 2) return null
                                val nx = (at.x / viewW * GestureMap.NORM).roundToInt().coerceIn(0, GestureMap.NORM)
                                val ny = (at.y / viewH * GestureMap.NORM).roundToInt().coerceIn(0, GestureMap.NORM)
                                return nx to ny
                            }
                            return if (trackpad) {
                                GestureMap.viewToNorm(GestureMap.letterbox(viewW, viewH, viewW, viewH), at.x, at.y)
                            } else {
                                PictureView.screenToNorm(
                                    viewW, viewH, session.frameW, session.frameH, picture, at.x, at.y,
                                )
                            }
                        }
                        if (!doubleTap && !viewOnly) {
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
                                    dropPointer()
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
                                        dropPointer()
                                        sawDown = false
                                    }
                                } else if (trackpad && kotlin.math.abs(span / startSpan - 1f) < 0.08f && !viewOnly) {
                                    scrolling = true
                                    val p1 = pointOf(a)
                                    val p2 = pointOf(b)
                                    if (p1 != null && p2 != null) {
                                        if (!sawDown) {
                                            pointer.down(pressed[0].uptimeMillis, listOf(p1, p2))
                                            sawDown = true
                                        } else {
                                            pointer.move(listOf(p1, p2)).forEach { send(it) }
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
                                onActivity()
                                val localPan = !trackpad && picture.scale > 1.05f && !pointer.dragLock && !pointer.dragArmed
                                if (localPan && !panning) {
                                    val travel = hypot(
                                        (at.x - first.position.x).toDouble(),
                                        (at.y - first.position.y).toDouble(),
                                    ).toFloat()
                                    if (travel > 24f) {
                                        if (sawDown) {
                                            dropPointer()
                                            sawDown = false
                                        }
                                        panning = true
                                    }
                                }
                                if (panning) {
                                    val placed = GestureMap.letterbox(
                                        session.frameW.coerceAtLeast(16),
                                        session.frameH.coerceAtLeast(9),
                                        viewW,
                                        viewH,
                                    )
                                    picture = PictureView.clamp(
                                        viewW,
                                        viewH,
                                        placed.width,
                                        placed.height,
                                        picture.copy(panX = picture.panX + (at.x - lastAt.x), panY = picture.panY + (at.y - lastAt.y)),
                                    )
                                } else if (!viewOnly) {
                                    val point = pointOf(at)
                                    if (point != null) {
                                        if (!sawDown) {
                                            pointer.down(pressed[0].uptimeMillis, listOf(point))
                                            sawDown = true
                                        } else {
                                            pointer.move(listOf(point)).forEach { send(it) }
                                            pointer.poll(pressed[0].uptimeMillis)?.let { send(it) }
                                        }
                                        cursor = if (trackpad) pointer.cursorNorm else point
                                        if (trackpad && picture.scale > 1.05f) {
                                            picture = PictureView.followCursor(
                                                viewW, viewH, session.frameW, session.frameH, picture, cursor,
                                            )
                                        }
                                    }
                                }
                                lastAt = at
                                event.changes.forEach { it.consume() }
                            }
                            if (pressed.isEmpty()) {
                                if (fingers >= 3 && trackpad && !viewOnly) {
                                    onMouse("down", cursor.first, cursor.second, 0, "middle")
                                    onMouse("up", cursor.first, cursor.second, 0, "middle")
                                } else if (startSpan > 0f && moved < 36f && !trackpad && !scrolling && !viewOnly) {
                                    val point = if (stretch) {
                                        pointOf(lastCentroid)
                                    } else {
                                        PictureView.screenToNorm(
                                            viewW, viewH, session.frameW, session.frameH, picture,
                                            lastCentroid.x, lastCentroid.y,
                                        )
                                    }
                                    if (point != null) {
                                        onMouse("down", point.first, point.second, 0, "right")
                                        onMouse("up", point.first, point.second, 0, "right")
                                    }
                                } else if ((scrolling || !zooming) && !doubleTap && sawDown) {
                                    pointer.up(event.changes.firstOrNull()?.uptimeMillis ?: now).forEach { send(it) }
                                    cursor = if (trackpad) pointer.cursorNorm else cursor
                                    lastTap = event.changes.firstOrNull()?.uptimeMillis ?: now
                                }
                                dropPointer()
                                break
                            }
                        }
                    }
                },
        ) {
            if (remote.showLocalCursor) {
                val placed = GestureMap.letterbox(
                    session.frameW.coerceAtLeast(16),
                    session.frameH.coerceAtLeast(9),
                    viewW,
                    viewH,
                )
                val px: Float
                val py: Float
                if (remote.scale == "stretch") {
                    px = (cursor.first / GestureMap.NORM.toFloat()) * viewW
                    py = (cursor.second / GestureMap.NORM.toFloat()) * viewH
                } else {
                    val dw = placed.width * picture.scale
                    val dh = placed.height * picture.scale
                    val left = viewW / 2f + picture.panX - dw / 2f
                    val top = viewH / 2f + picture.panY - dh / 2f
                    px = left + (cursor.first / GestureMap.NORM.toFloat()) * dw
                    py = top + (cursor.second / GestureMap.NORM.toFloat()) * dh
                }
                CursorMark(px, py)
            }
        }
        Box(
            Modifier
                .fillMaxSize()
                .zIndex(2f)
                .windowInsetsPadding(bars),
        ) {
            if (remote.showKeyBar) {
                ModifierBar(
                    modifier = Modifier.align(Alignment.BottomCenter).reportPad(space, "keys"),
                    onText = { if (!remote.viewOnly) onText(it) },
                    onKey = { scan, down -> if (!remote.viewOnly) onKey(scan, down) },
                    onClose = { onRemote(remote.copy(showKeyBar = false)) },
                )
            }
            if (idleWarn) {
                Surface(
                    modifier = Modifier
                        .align(Alignment.TopCenter)
                        .padding(top = 8.dp)
                        .reportPad(space, "idle"),
                    shape = RoundedCornerShape(20.dp),
                    color = MaterialTheme.colorScheme.surface.copy(alpha = 0.96f),
                ) {
                    TextButton(onClick = onStay, modifier = Modifier.padding(horizontal = 8.dp)) {
                        Text("Disconnecting in 10s, tap to stay")
                    }
                }
            }
            if (remote.showStats && (session.fps > 0 || session.latencyMs >= 0)) {
                val latency = if (session.latencyMs >= 0) "${session.latencyMs} ms" else "—"
                Surface(
                    modifier = Modifier
                        .align(Alignment.TopCenter)
                        .padding(top = 8.dp)
                        .reportPad(space, "stats"),
                    shape = RoundedCornerShape(12.dp),
                    color = MaterialTheme.colorScheme.surface.copy(alpha = 0.9f),
                ) {
                    Text(
                        "${session.fps} fps · ${session.bitrateKbps} kbps · $latency",
                        modifier = Modifier.padding(horizontal = 10.dp, vertical = 4.dp),
                        style = MaterialTheme.typography.labelMedium,
                    )
                }
            }
            if (session.displays.size > 1) {
                Row(
                    Modifier
                        .align(Alignment.TopStart)
                        .padding(start = 8.dp, top = 8.dp)
                        .horizontalScroll(rememberScrollState())
                        .reportPad(space, "displays"),
                    horizontalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    session.displays.forEach { display ->
                        HitButton(
                            description = "Display ${display.index + 1}",
                            space = space,
                            padId = "display-${display.index}",
                            color = if (display.index == shownDisplay) {
                                MaterialTheme.colorScheme.primary
                            } else {
                                MaterialTheme.colorScheme.surface.copy(alpha = 0.92f)
                            },
                            pressedColor = MaterialTheme.colorScheme.primary,
                            onClick = {
                                shownDisplay = display.index
                                onDisplay(display.index)
                            },
                        ) {
                            Text("${display.index + 1}", style = MaterialTheme.typography.titleMedium)
                        }
                    }
                }
            }
            Row(
                Modifier
                    .align(Alignment.TopEnd)
                    .padding(top = 8.dp, end = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Column(horizontalAlignment = Alignment.End) {
                    Box {
                        HitButton(
                            description = if (chatTooOld) CHAT_NEEDS_TIP else "Chat",
                            space = space,
                            padId = "chat",
                            color = if (chatReady) {
                                MaterialTheme.colorScheme.surface.copy(alpha = 0.94f)
                            } else {
                                MaterialTheme.colorScheme.surface.copy(alpha = 0.55f)
                            },
                            pressedColor = MaterialTheme.colorScheme.primary,
                            enabled = chatReady,
                            onClick = { chatSheet = true },
                        ) { ChatMark(dim = !chatReady) }
                        if (chatUnread > 0) {
                            Surface(
                                modifier = Modifier.align(Alignment.TopEnd),
                                shape = CircleShape,
                                color = MaterialTheme.colorScheme.error,
                            ) {
                                Text(
                                    if (chatUnread > 9) "9+" else chatUnread.toString(),
                                    modifier = Modifier.padding(horizontal = 5.dp, vertical = 1.dp),
                                    color = MaterialTheme.colorScheme.onError,
                                    style = MaterialTheme.typography.labelSmall,
                                )
                            }
                        }
                    }
                    if (chatTooOld) {
                        Text(
                            CHAT_NEEDS_TIP,
                            modifier = Modifier.widthIn(max = 220.dp),
                            color = MaterialTheme.colorScheme.onSurface,
                            style = MaterialTheme.typography.labelSmall,
                            textAlign = androidx.compose.ui.text.style.TextAlign.End,
                        )
                    }
                }
                HitButton(
                    description = "Session settings",
                    space = space,
                    padId = "gear",
                    color = MaterialTheme.colorScheme.surface.copy(alpha = 0.94f),
                    pressedColor = MaterialTheme.colorScheme.primary,
                    onClick = { sheet = true },
                ) { GearMark() }
                HitButton(
                    description = "End session",
                    space = space,
                    padId = "end",
                    color = Color(0xFFC62828),
                    pressedColor = Color(0xFF8E0000),
                    onClick = { confirmEnd = true },
                    onLongClick = onEnd,
                ) { EndMark() }
            }
            HitButton(
                description = "Session menu",
                modifier = Modifier
                    .align(Alignment.BottomCenter)
                    .padding(bottom = 8.dp),
                space = space,
                padId = "menu",
                color = MaterialTheme.colorScheme.surface.copy(alpha = 0.94f),
                pressedColor = MaterialTheme.colorScheme.primary,
                onClick = { sheet = true },
            ) { Mark("tools") }
            if (sheet) {
                Box(
                    Modifier
                        .fillMaxSize()
                        .background(Color.Black.copy(alpha = 0.45f))
                        .reportPad(space, "scrim")
                        .clickable { sheet = false },
                )
                Surface(
                    modifier = Modifier
                        .align(Alignment.BottomCenter)
                        .fillMaxWidth()
                        .fillMaxHeight(0.88f)
                        .reportPad(space, "sheet"),
                    shape = RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp),
                    color = MaterialTheme.colorScheme.surface,
                    tonalElevation = 8.dp,
                ) {
                    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(peerName, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                            TextButton(
                                onClick = { sheet = false },
                                modifier = Modifier.semantics { contentDescription = "Close session settings" },
                            ) { Text("Close") }
                        }
                        TextButton(
                            onClick = {
                                if (chatReady) {
                                    sheet = false
                                    chatSheet = true
                                }
                            },
                            enabled = chatReady,
                        ) { Text(if (chatTooOld) CHAT_NEEDS_TIP else "Chat") }
                        RemoteOptionsForm(
                            options = remote,
                            onChange = onRemote,
                            permissions = session.permissions,
                            peerName = peerName,
                            forThisComputer = forThisComputer,
                            onForThisComputer = onForThisComputer,
                            showActions = true,
                            fileNames = fileNames,
                            onClipboardNow = onClipboard,
                            onFileList = onFileList,
                            onSendFile = onSendFile,
                            onRestart = onRestart,
                            onLock = { onSpecial(1) },
                            onCtrlAltDel = { onSpecial(0) },
                            onDisconnect = {
                                sheet = false
                                confirmEnd = true
                            },
                        )
                    }
                }
            }
        }
        if (chatSheet) {
            Box(
                Modifier
                    .fillMaxSize()
                    .background(Color.Black.copy(alpha = 0.45f))
                    .clickable { chatSheet = false },
            )
            Box(Modifier.align(Alignment.BottomCenter)) {
                ChatSheet(
                    lines = chatLines,
                    ready = chatReady,
                    tooOld = chatTooOld,
                    onSend = onChat,
                    onClose = { chatSheet = false },
                )
            }
        }
        if (reconnecting) {
            Box(
                Modifier
                    .fillMaxSize()
                    .zIndex(30f)
                    .background(Color.Black.copy(alpha = 0.55f)),
                contentAlignment = Alignment.Center,
            ) {
                Surface(
                    shape = RoundedCornerShape(16.dp),
                    color = MaterialTheme.colorScheme.surface,
                    modifier = Modifier.padding(24.dp).widthIn(max = 420.dp),
                ) {
                    Column(Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        Text("Connection lost, reconnecting…", style = MaterialTheme.typography.titleMedium)
                        Text(if (retryLeft <= 0) "Trying again now…" else "Trying again in $retryLeft s.")
                        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            TextButton(onClick = onReconnectNow) { Text("Reconnect now") }
                            TextButton(onClick = onEnd) { Text("Close") }
                        }
                    }
                }
            }
        }
        if (confirmEnd) {
            AlertDialog(
                onDismissRequest = { confirmEnd = false },
                title = { Text("Disconnect from $peerName?") },
                confirmButton = {
                    TextButton(onClick = { confirmEnd = false; onEnd() }) { Text("Disconnect") }
                },
                dismissButton = {
                    TextButton(onClick = { confirmEnd = false }) { Text("Stay") }
                },
            )
        }
    }
}

@Composable
private fun HitButton(
    description: String,
    modifier: Modifier = Modifier,
    space: OverlaySpace? = null,
    padId: String? = null,
    color: Color,
    pressedColor: Color,
    enabled: Boolean = true,
    onClick: () -> Unit,
    onLongClick: (() -> Unit)? = null,
    icon: @Composable () -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val located = if (space != null && padId != null) Modifier.reportPad(space, padId) else Modifier
    val gesture = if (!enabled) {
        Modifier
    } else if (onLongClick == null) {
        Modifier.clickable(
            interactionSource = interaction,
            indication = ripple(bounded = false),
            onClick = onClick,
        )
    } else {
        Modifier.combinedClickable(
            interactionSource = interaction,
            indication = ripple(bounded = false),
            onClick = onClick,
            onLongClick = onLongClick,
        )
    }
    Surface(
        modifier = modifier
            .size(56.dp)
            .then(located)
            .semantics { contentDescription = description }
            .then(gesture),
        shape = CircleShape,
        color = if (pressed) pressedColor else color,
        border = if (pressed) BorderStroke(3.dp, Color.White) else null,
        tonalElevation = 6.dp,
    ) {
        Box(contentAlignment = Alignment.Center) { icon() }
    }
}

@Composable
private fun Modifier.reportPad(space: OverlaySpace, id: String): Modifier {
    DisposableEffect(id) {
        onDispose { space.pads.remove(id) }
    }
    return this.onGloballyPositioned { coords ->
        val b = coords.boundsInRoot()
        val o = space.origin
        space.pads[id] = ControlPad(b.left - o.x, b.top - o.y, b.right - o.x, b.bottom - o.y)
    }
}

private fun Modifier.onSizeChangedSafe(block: (androidx.compose.ui.unit.IntSize) -> Unit): Modifier =
    onGloballyPositioned { block(it.size) }

/** X mark for End, the same stroke icon as the Windows toolbar. */
@Composable
private fun EndMark() {
    Canvas(Modifier.size(22.dp)) {
        val w = 2.2.dp.toPx()
        val u = size.minDimension / 18f
        val cap = androidx.compose.ui.graphics.StrokeCap.Round
        drawLine(Color.White, Offset(4.5f * u, 4.5f * u), Offset(13.5f * u, 13.5f * u), strokeWidth = w, cap = cap)
        drawLine(Color.White, Offset(13.5f * u, 4.5f * u), Offset(4.5f * u, 13.5f * u), strokeWidth = w, cap = cap)
    }
}

/** Chat bubble stroke icon. */
@Composable
private fun ChatMark(dim: Boolean) {
    val color = MaterialTheme.colorScheme.onSurface.copy(alpha = if (dim) 0.45f else 1f)
    Canvas(Modifier.size(22.dp)) {
        val w = 1.8.dp.toPx()
        val u = size.minDimension / 18f
        drawRoundRect(
            color,
            topLeft = Offset(1.5f * u, 2.5f * u),
            size = androidx.compose.ui.geometry.Size(15f * u, 10.5f * u),
            cornerRadius = androidx.compose.ui.geometry.CornerRadius(3f * u, 3f * u),
            style = Stroke(width = w),
        )
        val cap = androidx.compose.ui.graphics.StrokeCap.Round
        drawLine(color, Offset(5f * u, 13f * u), Offset(4.6f * u, 16.4f * u), strokeWidth = w, cap = cap)
        drawLine(color, Offset(4.6f * u, 16.4f * u), Offset(9f * u, 13f * u), strokeWidth = w, cap = cap)
    }
}

@Composable
private fun GearMark() {
    val color = MaterialTheme.colorScheme.onSurface
    Canvas(Modifier.size(22.dp)) {
        drawCircle(color, radius = size.minDimension * 0.22f, style = Stroke(width = 1.8.dp.toPx()))
        drawCircle(color, radius = size.minDimension * 0.08f)
        val arm = size.minDimension * 0.42f
        for (i in 0 until 8) {
            val angle = (Math.PI * i / 4.0)
            val c = kotlin.math.cos(angle).toFloat()
            val s = kotlin.math.sin(angle).toFloat()
            drawLine(
                color,
                Offset(center.x + c * size.minDimension * 0.28f, center.y + s * size.minDimension * 0.28f),
                Offset(center.x + c * arm, center.y + s * arm),
                strokeWidth = 1.8.dp.toPx(),
            )
        }
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
                    androidx.compose.material3.FilterChip(
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
                    androidx.compose.material3.FilterChip(
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
                androidx.compose.material3.OutlinedTextField(
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
                androidx.compose.material3.IconButton(onClick = { onText("\n"); typed = "" }) { Mark("enter") }
                androidx.compose.material3.IconButton(onClick = onClose) { Mark("end") }
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
