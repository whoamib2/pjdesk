package us.spader.pjdesk.ui

import androidx.compose.foundation.layout.RowScope
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.vector.ImageVector

@Composable
fun PjBottomBar(selected: Int, onSelect: (Int) -> Unit) {
    val colors = NavigationBarItemDefaults.colors(
        selectedIconColor = MaterialTheme.colorScheme.primary,
        selectedTextColor = MaterialTheme.colorScheme.primary,
        unselectedIconColor = pjMuted(),
        unselectedTextColor = pjMuted(),
        indicatorColor = MaterialTheme.colorScheme.primary.copy(alpha = 0.16f),
    )
    NavigationBar(containerColor = MaterialTheme.colorScheme.surface) {
        NavItem(0, selected, "Home", PjIcons.Home, colors, onSelect)
        NavItem(1, selected, "Saved", PjIcons.Saved, colors, onSelect)
        NavItem(2, selected, "Settings", PjIcons.Settings, colors, onSelect)
    }
}

@Composable
private fun RowScope.NavItem(
    index: Int,
    selected: Int,
    label: String,
    icon: ImageVector,
    colors: androidx.compose.material3.NavigationBarItemColors,
    onSelect: (Int) -> Unit,
) {
    NavigationBarItem(
        selected = selected == index,
        onClick = { onSelect(index) },
        icon = { Icon(icon, contentDescription = label) },
        label = { Text(label) },
        alwaysShowLabel = true,
        colors = colors,
    )
}
