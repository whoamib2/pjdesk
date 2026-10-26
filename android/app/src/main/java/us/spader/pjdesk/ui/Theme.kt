package us.spader.pjdesk.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import us.spader.pjdesk.R

val AccentDark = Color(0xFF4D9BFF)
val AccentLight = Color(0xFF2F6FED)
val OkDark = Color(0xFF46D17D)
val OkLight = Color(0xFF1C8A4C)
val WarnDark = Color(0xFFF2B03A)
val WarnLight = Color(0xFFB46B00)
val TextDark = Color(0xFFE9EDF3)
val TextLight = Color(0xFF1C2330)
val MutedDark = Color(0xFF9AA3B5)
val MutedLight = Color(0xFF5C6778)
val PanelDark = Color(0xFF101318)
val PanelLight = Color(0xFFF3F5F8)
val CardDark = Color(0xFF181C24)
val CardLight = Color(0xFFFFFFFF)
val ElevDark = Color(0xFF222833)
val ElevLight = Color(0xFFE8ECF2)
val LineDark = Color(0xFF2E3644)
val LineLight = Color(0xFFD5DBE3)

val Inter = FontFamily(
    Font(R.font.inter_regular, FontWeight.Normal),
    Font(R.font.inter_semibold, FontWeight.SemiBold),
)

private val DarkColors = darkColorScheme(
    primary = AccentDark,
    onPrimary = Color.White,
    secondary = AccentDark,
    background = PanelDark,
    onBackground = TextDark,
    surface = CardDark,
    onSurface = TextDark,
    surfaceVariant = ElevDark,
    onSurfaceVariant = MutedDark,
    secondaryContainer = Color(0xFF243044),
    onSecondaryContainer = TextDark,
    outline = LineDark,
    error = WarnDark,
)

private val LightColors = lightColorScheme(
    primary = AccentLight,
    onPrimary = Color.White,
    secondary = AccentLight,
    background = PanelLight,
    onBackground = TextLight,
    surface = CardLight,
    onSurface = TextLight,
    surfaceVariant = ElevLight,
    onSurfaceVariant = MutedLight,
    secondaryContainer = Color(0xFFD9E6FF),
    onSecondaryContainer = TextLight,
    outline = LineLight,
    error = WarnLight,
)

private val Type = Typography(
    headlineMedium = TextStyle(fontFamily = Inter, fontWeight = FontWeight.SemiBold, fontSize = 22.sp),
    titleLarge = TextStyle(fontFamily = Inter, fontWeight = FontWeight.SemiBold, fontSize = 20.sp),
    titleMedium = TextStyle(fontFamily = Inter, fontWeight = FontWeight.SemiBold, fontSize = 16.sp),
    bodyLarge = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Normal, fontSize = 16.sp),
    bodyMedium = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Normal, fontSize = 14.5.sp),
    bodySmall = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Normal, fontSize = 13.sp),
    labelLarge = TextStyle(fontFamily = Inter, fontWeight = FontWeight.SemiBold, fontSize = 14.5.sp),
    labelMedium = TextStyle(fontFamily = Inter, fontWeight = FontWeight.Normal, fontSize = 12.5.sp),
    labelSmall = TextStyle(fontFamily = Inter, fontWeight = FontWeight.SemiBold, fontSize = 11.sp),
)

@Composable
fun PjTheme(theme: String = "system", content: @Composable () -> Unit) {
    val dark = when (theme) {
        "dark" -> true
        "light" -> false
        else -> isSystemInDarkTheme()
    }
    MaterialTheme(
        colorScheme = if (dark) DarkColors else LightColors,
        typography = Type,
        content = content,
    )
}

@Composable
fun pjMuted(): Color = MaterialTheme.colorScheme.onSurfaceVariant

@Composable
fun pjOk(): Color = if (isSystemInDarkTheme()) OkDark else OkLight

@Composable
fun pjWarn(): Color = if (isSystemInDarkTheme()) WarnDark else WarnLight
