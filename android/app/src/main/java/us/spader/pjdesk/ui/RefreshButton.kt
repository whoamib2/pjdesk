package us.spader.pjdesk.ui

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.PlainTooltip
import androidx.compose.material3.Text
import androidx.compose.material3.TooltipBox
import androidx.compose.material3.TooltipDefaults
import androidx.compose.material3.rememberTooltipState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import kotlinx.coroutines.launch

/** How long the refresh mark spins after a tap, in milliseconds. */
const val REFRESH_SPIN_MS = 450

/**
 * The "new password" button: the vector circular arrow, the same size as the
 * copy icon, a pressed highlight, the tooltip "New password" on long press,
 * and one short spin when tapped.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RefreshPasswordButton(onClick: () -> Unit, enabled: Boolean = true, tip: String = "New password") {
    val turn = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    TooltipBox(
        positionProvider = TooltipDefaults.rememberPlainTooltipPositionProvider(),
        tooltip = { PlainTooltip { Text(tip) } },
        state = rememberTooltipState(),
    ) {
        IconButton(
            onClick = {
                onClick()
                scope.launch {
                    turn.snapTo(0f)
                    turn.animateTo(360f, tween(REFRESH_SPIN_MS, easing = FastOutSlowInEasing))
                    turn.snapTo(0f)
                }
            },
            enabled = enabled,
        ) {
            Icon(
                PjIcons.Refresh,
                contentDescription = tip,
                tint = MaterialTheme.colorScheme.onSurface,
                modifier = Modifier.rotate(turn.value),
            )
        }
    }
}

/** The copy icon button that sits beside it. */
@Composable
fun CopyIconButton(onClick: () -> Unit, description: String) {
    IconButton(onClick = onClick) {
        Icon(PjIcons.Copy, contentDescription = description, tint = MaterialTheme.colorScheme.onSurface)
    }
}
