package us.spader.pjdesk.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp

/** Same shapes as the Windows client's OS icons. Android is a phone outline. */
@Composable
fun OsIcon(family: String, modifier: Modifier = Modifier, tint: Color = Color.Unspecified) {
    val color = if (tint == Color.Unspecified) androidx.compose.material3.MaterialTheme.colorScheme.onSurface else tint
    Canvas(modifier.size(24.dp)) {
        val s = size.minDimension
        val c = Offset(size.width / 2f, size.height / 2f)
        val stroke = Stroke(width = s * 0.08f)
        when (family) {
            "windows" -> {
                val w = s * 0.34f
                val h = s * 0.28f
                val gap = s * 0.06f
                fun pane(dx: Float, dy: Float) {
                    drawRect(color, topLeft = c + Offset(dx, dy), size = Size(w, h), style = stroke)
                }
                pane(-w - gap / 2, -h - gap / 2)
                pane(gap / 2, -h - gap / 2)
                pane(-w - gap / 2, gap / 2)
                pane(gap / 2, gap / 2)
            }
            "macos" -> {
                val w = s * 0.74f
                val h = s * 0.56f
                drawRoundRect(
                    color,
                    topLeft = Offset(c.x - w / 2, c.y - h / 2 + s * 0.04f),
                    size = Size(w, h),
                    cornerRadius = CornerRadius(s * 0.08f),
                    style = stroke,
                )
            }
            "linux" -> {
                drawLine(color, Offset(c.x - s * 0.28f, c.y - s * 0.16f), Offset(c.x - s * 0.28f, c.y), stroke.width)
                drawLine(color, Offset(c.x - s * 0.28f, c.y), Offset(c.x - s * 0.12f, c.y + s * 0.14f), stroke.width)
                drawLine(color, Offset(c.x + s * 0.02f, c.y + s * 0.14f), Offset(c.x + s * 0.28f, c.y + s * 0.14f), stroke.width)
            }
            "android" -> {
                val w = s * 0.42f
                val h = s * 0.68f
                val left = c.x - w / 2
                val top = c.y - h / 2 + s * 0.04f
                drawRoundRect(
                    color,
                    topLeft = Offset(left, top),
                    size = Size(w, h),
                    cornerRadius = CornerRadius(s * 0.08f),
                    style = stroke,
                )
                drawLine(color, Offset(left + s * 0.08f, top + s * 0.16f), Offset(left + w - s * 0.08f, top + s * 0.16f), stroke.width)
            }
            else -> {
                drawCircle(color, radius = s * 0.28f, center = c, style = stroke)
                drawLine(color, c + Offset(0f, -s * 0.08f), c + Offset(0f, s * 0.02f), stroke.width)
                drawCircle(color, radius = s * 0.04f, center = c + Offset(0f, s * 0.12f))
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
