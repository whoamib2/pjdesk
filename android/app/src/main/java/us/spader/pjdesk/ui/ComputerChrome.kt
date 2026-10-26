package us.spader.pjdesk.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.computerTitle
import us.spader.pjdesk.core.savedSubtitle
import us.spader.pjdesk.core.shownAppVersion
import us.spader.pjdesk.core.versionBehind
import us.spader.pjdesk.core.tileCaption
import us.spader.pjdesk.core.tileKey
import us.spader.pjdesk.core.tileRgb

private fun versioned(computer: SavedComputer, mine: Boolean, selfVersion: String): SavedComputer {
    if (!mine || computer.appVersion.isNotBlank() || selfVersion.isBlank()) return computer
    return computer.copy(appVersion = selfVersion)
}

@Composable
private fun VersionSubtitle(computer: SavedComputer, mine: Boolean, latestRelease: String) {
    val text = savedSubtitle(computer, mine = mine)
    val version = shownAppVersion(computer.appVersion)
    val behind = versionBehind(computer.appVersion, latestRelease)
    if (!behind || !text.endsWith(version)) {
        Text(
            text,
            color = pjMuted(),
            style = MaterialTheme.typography.bodySmall,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        return
    }
    Text(
        buildAnnotatedString {
            withStyle(SpanStyle(color = pjMuted())) { append(text.removeSuffix(version)) }
            withStyle(SpanStyle(color = pjWarn())) { append(version) }
        },
        style = MaterialTheme.typography.bodySmall,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
}

fun computerTileColor(computer: SavedComputer): Color {
    val rgb = tileRgb(tileKey(computer.id, computer.hostname))
    return Color((0xFF000000L or rgb.toLong()).toInt())
}

@Composable
fun ComputerAvatar(
    computer: SavedComputer,
    online: Boolean,
    modifier: Modifier = Modifier,
    size: Dp = 40.dp,
) {
    val color = computerTileColor(computer)
    Box(modifier.size(size)) {
        Box(
            Modifier
                .fillMaxSize()
                .clip(RoundedCornerShape(size * 0.24f))
                .background(color),
            contentAlignment = Alignment.Center,
        ) {
            OsIcon(
                computer.osFamily.ifBlank { "unknown" },
                Modifier.size(size * 0.62f),
                tint = Color.White,
                cutout = color,
            )
        }
        StatusBadge(
            online = online,
            modifier = Modifier
                .align(Alignment.BottomEnd)
                .offset(2.dp, 2.dp),
        )
    }
}

@Composable
fun StatusBadge(online: Boolean, modifier: Modifier = Modifier) {
    val dark = MaterialTheme.colorScheme.background.luminance() < 0.5f
    val dot = if (online) {
        if (dark) OkDark else OkLight
    } else {
        pjMuted()
    }
    Box(modifier.size(14.dp), contentAlignment = Alignment.Center) {
        Box(
            Modifier
                .size(14.dp)
                .clip(CircleShape)
                .background(MaterialTheme.colorScheme.background),
        )
        Box(
            Modifier
                .size(10.dp)
                .clip(CircleShape)
                .background(dot),
        )
    }
}

@Composable
fun ThisPhoneChip() {
    Text(
        "This phone",
        color = MaterialTheme.colorScheme.primary,
        style = MaterialTheme.typography.labelSmall,
        maxLines = 1,
        modifier = Modifier
            .clip(RoundedCornerShape(50))
            .background(MaterialTheme.colorScheme.primary.copy(alpha = 0.16f))
            .padding(horizontal = 8.dp, vertical = 2.dp),
    )
}

@Composable
fun ConnectButton(onClick: () -> Unit) {
    Box(
        Modifier
            .size(48.dp)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Box(
            Modifier
                .size(34.dp)
                .clip(CircleShape)
                .background(MaterialTheme.colorScheme.primary),
            contentAlignment = Alignment.Center,
        ) {
            androidx.compose.foundation.Canvas(Modifier.size(16.dp)) {
                val path = Path().apply {
                    moveTo(size.width * 0.28f, size.height * 0.14f)
                    lineTo(size.width * 0.86f, size.height * 0.50f)
                    lineTo(size.width * 0.28f, size.height * 0.86f)
                    close()
                }
                drawPath(path, Color.White)
            }
        }
    }
}

@Composable
fun ComputerRow(
    computer: SavedComputer,
    mine: Boolean,
    onConnect: () -> Unit,
    showMenu: Boolean = true,
    inset: Dp = 12.dp,
    onRename: () -> Unit = {},
    onRemovePassword: () -> Unit = {},
    onForget: () -> Unit = {},
    latestRelease: String = "",
    selfVersion: String = "",
) {
    val title = computerTitle(computer)
    val shown = versioned(computer, mine, selfVersion)
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .clickable(enabled = !mine, onClick = onConnect)
            .padding(horizontal = inset, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ComputerAvatar(computer, online = mine || computer.online, size = 40.dp)
        Column(
            Modifier
                .weight(1f)
                .padding(horizontal = 10.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(
                    title,
                    style = MaterialTheme.typography.bodyLarge,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                    color = if (title == "No name yet") pjMuted() else MaterialTheme.colorScheme.onSurface,
                )
                if (mine) ThisPhoneChip()
            }
            VersionSubtitle(shown, mine, latestRelease)
        }
        if (!mine) ConnectButton(onConnect)
        if (showMenu) {
            ComputerMenu(computer, onRename, onRemovePassword, onForget)
        }
    }
}

@Composable
fun ComputerCard(
    computer: SavedComputer,
    mine: Boolean,
    big: Boolean,
    onConnect: () -> Unit,
    onRename: () -> Unit = {},
    onRemovePassword: () -> Unit = {},
    onForget: () -> Unit = {},
    latestRelease: String = "",
    selfVersion: String = "",
) {
    val color = computerTileColor(computer)
    val shown = versioned(computer, mine, selfVersion)
    val title = computerTitle(computer)
    Surface(
        modifier = Modifier
            .padding(6.dp)
            .fillMaxWidth()
            .clickable(enabled = !mine, onClick = onConnect),
        shape = RoundedCornerShape(12.dp),
        color = MaterialTheme.colorScheme.surface,
    ) {
        Column {
            Box(
                Modifier
                    .fillMaxWidth()
                    .height(if (big) 132.dp else 88.dp)
                    .background(color),
                contentAlignment = Alignment.Center,
            ) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    OsIcon(
                        computer.osFamily.ifBlank { "unknown" },
                        Modifier.size(if (big) 56.dp else 32.dp),
                        tint = Color.White,
                        cutout = color,
                    )
                    if (big) {
                        Text(
                            tileCaption(computer),
                            color = Color.White,
                            style = MaterialTheme.typography.bodySmall,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            textAlign = TextAlign.Center,
                            modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
                        )
                        Text(
                            shownAppVersion(shown.appVersion),
                            color = if (versionBehind(shown.appVersion, latestRelease)) pjWarn() else Color.White,
                            style = MaterialTheme.typography.labelSmall,
                            maxLines = 1,
                        )
                    }
                }
                StatusBadge(
                    online = mine || computer.online,
                    modifier = Modifier
                        .align(Alignment.BottomEnd)
                        .padding(8.dp),
                )
            }
            Row(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 48.dp)
                    .padding(start = 10.dp, end = 2.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Column(Modifier.weight(1f)) {
                    Text(
                        title,
                        style = MaterialTheme.typography.bodyMedium,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        color = if (title == "No name yet") pjMuted() else MaterialTheme.colorScheme.onSurface,
                    )
                    Text(
                        shownAppVersion(shown.appVersion),
                        color = if (versionBehind(shown.appVersion, latestRelease)) pjWarn() else pjMuted(),
                        style = MaterialTheme.typography.labelSmall,
                        maxLines = 1,
                    )
                }
                if (mine) {
                    ThisPhoneChip()
                } else {
                    ConnectButton(onConnect)
                }
                ComputerMenu(computer, onRename, onRemovePassword, onForget)
            }
        }
    }
}

@Composable
private fun ComputerMenu(
    computer: SavedComputer,
    onRename: () -> Unit,
    onRemovePassword: () -> Unit,
    onForget: () -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    val title = computerTitle(computer)
    val user = computer.consoleUser.trim()
    val host = computer.hostname.trim()
    Box {
        Box(
            Modifier
                .size(40.dp)
                .clickable { menu = true },
            contentAlignment = Alignment.Center,
        ) {
            Icon(PjIcons.More, contentDescription = "More actions", tint = pjMuted())
        }
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            if (user.isNotBlank()) {
                DropdownMenuItem(text = { Text("User: $user") }, onClick = { menu = false }, enabled = false)
            }
            if (host.isNotBlank() && !host.equals(title, ignoreCase = true)) {
                DropdownMenuItem(text = { Text(host) }, onClick = { menu = false }, enabled = false)
            }
            DropdownMenuItem(text = { Text("Rename") }, onClick = { menu = false; onRename() })
            if (computer.passwordSaved) {
                DropdownMenuItem(text = { Text("Remove password") }, onClick = { menu = false; onRemovePassword() })
            }
            DropdownMenuItem(text = { Text("Remove") }, onClick = { menu = false; onForget() })
        }
    }
}
