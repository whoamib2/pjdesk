package us.spader.pjdesk.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp

/**
 * White platform mark drawn on a colored tile. [cutout] is the tile color, so a
 * screen reads as a hole in the white shape. Windows is four panes. Unknown is
 * a monitor.
 */
@Composable
fun OsIcon(
    family: String,
    modifier: Modifier = Modifier,
    tint: Color = Color.White,
    cutout: Color = Color.Black,
) {
    Canvas(modifier.size(24.dp)) {
        val s = size.minDimension
        val c = Offset(size.width / 2f, size.height / 2f)
        when (family.lowercase()) {
            "windows" -> {
                val gap = s * 0.08f
                val pane = (s * 0.82f - gap) / 2f
                val left = (size.width - (pane * 2f + gap)) / 2f
                val top = (size.height - (pane * 2f + gap)) / 2f
                val radius = CornerRadius(s * 0.08f)
                for (row in 0..1) {
                    for (col in 0..1) {
                        drawRoundRect(
                            tint,
                            topLeft = Offset(left + col * (pane + gap), top + row * (pane + gap)),
                            size = Size(pane, pane),
                            cornerRadius = radius,
                        )
                    }
                }
            }
            "macos" -> {
                val sw = s * 0.70f
                val sh = s * 0.42f
                val left = c.x - sw / 2f
                val top = c.y - s * 0.36f
                drawRoundRect(tint, Offset(left, top), Size(sw, sh), CornerRadius(s * 0.08f))
                val inset = s * 0.07f
                drawRoundRect(
                    cutout,
                    Offset(left + inset, top + inset),
                    Size(sw - inset * 2f, sh - inset * 2.2f),
                    CornerRadius(s * 0.04f),
                )
                drawLine(
                    tint,
                    Offset(c.x - s * 0.40f, top + sh + s * 0.08f),
                    Offset(c.x + s * 0.40f, top + sh + s * 0.08f),
                    strokeWidth = s * 0.09f,
                    cap = StrokeCap.Round,
                )
            }
            "linux" -> {
                val sw = s * 0.78f
                val sh = s * 0.58f
                val left = c.x - sw / 2f
                val top = c.y - sh / 2f
                drawRoundRect(tint, Offset(left, top), Size(sw, sh), CornerRadius(s * 0.1f))
                val inset = s * 0.08f
                drawRoundRect(
                    cutout,
                    Offset(left + inset, top + inset),
                    Size(sw - inset * 2f, sh - inset * 2f),
                    CornerRadius(s * 0.05f),
                )
                val stroke = Stroke(width = s * 0.07f, cap = StrokeCap.Round)
                val ox = left + inset * 2f
                val oy = c.y
                drawLine(tint, Offset(ox, oy - s * 0.1f), Offset(ox + s * 0.12f, oy), stroke.width, cap = StrokeCap.Round)
                drawLine(tint, Offset(ox + s * 0.12f, oy), Offset(ox, oy + s * 0.1f), stroke.width, cap = StrokeCap.Round)
                drawLine(tint, Offset(ox + s * 0.2f, oy + s * 0.1f), Offset(ox + s * 0.38f, oy + s * 0.1f), stroke.width, cap = StrokeCap.Round)
            }
            "android" -> {
                val w = s * 0.46f
                val h = s * 0.78f
                val left = c.x - w / 2f
                val top = c.y - h / 2f
                drawRoundRect(tint, Offset(left, top), Size(w, h), CornerRadius(s * 0.12f))
                val inset = s * 0.07f
                drawRoundRect(
                    cutout,
                    Offset(left + inset, top + inset * 1.7f),
                    Size(w - inset * 2f, h - inset * 3.4f),
                    CornerRadius(s * 0.04f),
                )
                drawCircle(tint, radius = s * 0.035f, center = Offset(c.x, top + h - inset * 1.05f))
            }
            else -> {
                val sw = s * 0.78f
                val sh = s * 0.50f
                val left = c.x - sw / 2f
                val top = c.y - s * 0.40f
                drawRoundRect(tint, Offset(left, top), Size(sw, sh), CornerRadius(s * 0.1f))
                val inset = s * 0.08f
                drawRoundRect(
                    cutout,
                    Offset(left + inset, top + inset),
                    Size(sw - inset * 2f, sh - inset * 2f),
                    CornerRadius(s * 0.04f),
                )
                val neck = top + sh
                drawLine(tint, Offset(c.x, neck), Offset(c.x, neck + s * 0.14f), strokeWidth = s * 0.08f, cap = StrokeCap.Round)
                drawLine(
                    tint,
                    Offset(c.x - s * 0.2f, neck + s * 0.14f),
                    Offset(c.x + s * 0.2f, neck + s * 0.14f),
                    strokeWidth = s * 0.08f,
                    cap = StrokeCap.Round,
                )
            }
        }
    }
}

fun osLabel(family: String): String = when (family) {
    "windows" -> "Windows"
    "macos" -> "macOS"
    "linux" -> "Linux"
    "android" -> "Android"
    "" -> ""
    else -> family.replaceFirstChar { it.uppercase() }
}
