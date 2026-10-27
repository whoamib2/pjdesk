package us.spader.pjdesk.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.SubcomposeLayout
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import us.spader.pjdesk.AppLog
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.SavedFilter
import us.spader.pjdesk.core.SavedSort
import us.spader.pjdesk.core.SavedView
import us.spader.pjdesk.core.arrangeSaved
import us.spader.pjdesk.core.computerTitle
import us.spader.pjdesk.core.nickname
import us.spader.pjdesk.core.savedComputerCountLine
import us.spader.pjdesk.core.savedMatches

@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
fun SavedScreen(
    computers: List<SavedComputer>,
    query: String,
    onQuery: (String) -> Unit,
    onForget: (SavedComputer) -> Unit,
    onRemovePassword: (SavedComputer) -> Unit = {},
    onRename: (SavedComputer, String) -> Unit = { _, _ -> },
    onConnect: (SavedComputer) -> Unit = {},
    recentIds: List<String> = emptyList(),
    selfId: String = "",
    @Suppress("UNUSED_PARAMETER") signedIn: Boolean = false,
    status: String = "",
    sort: SavedSort = SavedSort.Recent,
    filter: SavedFilter = SavedFilter.All,
    onSort: (SavedSort) -> Unit = {},
    onFilter: (SavedFilter) -> Unit = {},
    view: SavedView = SavedView.List,
    onView: (SavedView) -> Unit = {},
    discovered: List<SavedComputer> = emptyList(),
    latestRelease: String = "",
    selfVersion: String = "",
    initialPage: Int = 0,
    refreshing: Boolean = false,
    onRefresh: () -> Unit = {},
    onRemoveAll: () -> Unit = {},
    onSignOutRemove: () -> Unit = {},
) {
    var page by remember { mutableStateOf(initialPage.coerceIn(0, 2)) }
    var confirmClear by remember { mutableStateOf(false) }
    var confirmSignOut by remember { mutableStateOf(false) }
    var searchOpen by remember { mutableStateOf(query.isNotBlank()) }
    var menu by remember { mutableStateOf(false) }
    val self = selfId.filter { it.isDigit() }
    val shown = when (page) {
        2 -> discovered
            .filter { savedMatches(it, query) }
            .sortedBy { computerTitle(it).lowercase() }
        else -> arrangeSaved(computers, recentIds, sort, filter, query, recentOnly = page == 1, selfId = selfId)
    }
    var editing by remember { mutableStateOf<SavedComputer?>(null) }
    var draft by remember { mutableStateOf("") }
    Column(Modifier.fillMaxSize()) {
        SavedChrome(
            page = page,
            onPage = { page = it },
            view = view,
            onView = onView,
            searchOpen = searchOpen,
            onSearch = { searchOpen = true },
            menu = menu,
            onMenu = { menu = it },
            sort = sort,
            filter = filter,
            onSort = onSort,
            onFilter = onFilter,
            savedPage = page == 0,
            onRemoveAll = { confirmClear = true },
            onSignOutRemove = { confirmSignOut = true },
        )
        if (searchOpen) {
            OutlinedTextField(
                value = query,
                onValueChange = onQuery,
                modifier = Modifier
                    .fillMaxWidth()
                    .height(52.dp)
                    .padding(horizontal = 12.dp),
                singleLine = true,
                textStyle = MaterialTheme.typography.bodyMedium,
                placeholder = { Text("Name, ID, or OS") },
                trailingIcon = {
                    TextButton(onClick = {
                        onQuery("")
                        searchOpen = false
                    }) { Text("Close") }
                },
            )
        }
        if (page == 2) {
            Text(
                "PJDesk computers on this network.",
                color = pjMuted(),
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 2.dp),
            )
        }
        if (status.isNotBlank()) {
            Row(
                Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                Text(status, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        PullToRefreshBox(
            isRefreshing = refreshing,
            onRefresh = onRefresh,
            modifier = Modifier.weight(1f),
        ) {
            if (shown.isEmpty()) {
                Text(
                    when {
                        page == 2 && query.isBlank() -> "No PJDesk computers have answered on this network yet."
                        computers.isEmpty() && query.isBlank() && filter == SavedFilter.All && page == 0 ->
                            "Computers you connect to show up here."
                        else -> "No computers match."
                    },
                    color = pjMuted(),
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                    style = MaterialTheme.typography.bodyMedium,
                )
            } else if (view == SavedView.List) {
                LazyColumn(modifier = Modifier.fillMaxSize()) {
                    items(shown, key = { it.id }) { computer ->
                        val mine = self.isNotBlank() && computer.id.filter { it.isDigit() } == self
                        ComputerRow(
                            computer = computer,
                            mine = mine,
                            onConnect = {
                                AppLog.i("Saved row Connect ${computer.id} online=${computer.online}")
                                onConnect(computer)
                            },
                            onRename = {
                                editing = computer
                                draft = computer.nickname()
                            },
                            onRemovePassword = { onRemovePassword(computer) },
                            onForget = { onForget(computer) },
                            latestRelease = latestRelease,
                            selfVersion = selfVersion,
                        )
                    }
                }
            } else {
                LazyVerticalGrid(
                    columns = GridCells.Fixed(if (view == SavedView.Big) 1 else 2),
                    modifier = Modifier
                        .fillMaxSize()
                        .padding(horizontal = 6.dp),
                ) {
                    items(shown, key = { it.id }) { computer ->
                        val mine = self.isNotBlank() && computer.id.filter { it.isDigit() } == self
                        ComputerCard(
                            computer = computer,
                            mine = mine,
                            big = view == SavedView.Big,
                            onConnect = {
                                AppLog.i("Saved tile Connect ${computer.id} online=${computer.online}")
                                onConnect(computer)
                            },
                            onRename = {
                                editing = computer
                                draft = computer.nickname()
                            },
                            onRemovePassword = { onRemovePassword(computer) },
                            onForget = { onForget(computer) },
                            latestRelease = latestRelease,
                            selfVersion = selfVersion,
                        )
                    }
                }
            }
        }
    }
    if (confirmClear) {
        LocalSavedClearDialog(
            title = "Remove all saved computers",
            count = computers.size,
            detail = "This clears the saved list, cached passwords, recent computers, and saved addresses on this computer only. Nothing is deleted from your pjdesk.com account or from your other computers.",
            confirm = "Remove from this computer",
            onDismiss = { confirmClear = false },
            onConfirm = {
                confirmClear = false
                onRemoveAll()
            },
        )
    }
    if (confirmSignOut) {
        LocalSavedClearDialog(
            title = "Sign out and remove everything",
            count = computers.size,
            detail = "This signs out on this computer and clears the saved list, cached passwords, recent computers, and saved addresses here. Nothing is deleted from your pjdesk.com account or from your other computers.",
            confirm = "Sign out and remove",
            onDismiss = { confirmSignOut = false },
            onConfirm = {
                confirmSignOut = false
                onSignOutRemove()
            },
        )
    }
    val renaming = editing
    if (renaming != null) {
        Dialog(onDismissRequest = { editing = null }) {
            Card(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text("Rename", style = MaterialTheme.typography.titleLarge)
                    OutlinedTextField(
                        value = draft,
                        onValueChange = { draft = it.take(64) },
                        modifier = Modifier.fillMaxWidth(),
                        singleLine = true,
                        label = { Text("Nickname") },
                    )
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                        TextButton(onClick = { editing = null }) { Text("Cancel") }
                        TextButton(onClick = {
                            onRename(renaming, draft.trim())
                            editing = null
                        }) { Text("Save") }
                    }
                }
            }
        }
    }
}

@Composable
private fun SavedChrome(
    page: Int,
    onPage: (Int) -> Unit,
    view: SavedView,
    onView: (SavedView) -> Unit,
    searchOpen: Boolean,
    onSearch: () -> Unit,
    menu: Boolean,
    onMenu: (Boolean) -> Unit,
    sort: SavedSort,
    filter: SavedFilter,
    onSort: (SavedSort) -> Unit,
    onFilter: (SavedFilter) -> Unit,
    savedPage: Boolean,
    onRemoveAll: () -> Unit,
    onSignOutRemove: () -> Unit,
) {
    SubcomposeLayout(Modifier.fillMaxWidth().heightIn(min = 48.dp)) { constraints ->
        val maxHeight = constraints.maxHeight.let { if (it == Constraints.Infinity) 10_000 else it }
        val loose = Constraints(maxWidth = 10_000, maxHeight = maxHeight)
        val tabs = subcompose("tabs") { TabStrip(page, onPage) }.first().measure(loose)
        val icons = subcompose("icons") {
            IconStrip(view, onView, searchOpen, onSearch, menu, onMenu, sort, filter, onSort, onFilter, savedPage, onRemoveAll, onSignOutRemove)
        }.first().measure(loose)
        val stacked = tabs.width + icons.width > constraints.maxWidth
        if (!stacked) {
            val height = maxOf(tabs.height, icons.height)
            layout(constraints.maxWidth, height) {
                tabs.place(0, (height - tabs.height) / 2)
                icons.place(constraints.maxWidth - icons.width, (height - icons.height) / 2)
            }
        } else {
            val tabPlace = subcompose("tabs-full") { TabStrip(page, onPage, Modifier.fillMaxWidth()) }
                .first()
                .measure(constraints.copy(minWidth = 0, minHeight = 0))
            val iconPlace = subcompose("icons-full") {
                IconStrip(view, onView, searchOpen, onSearch, menu, onMenu, sort, filter, onSort, onFilter, savedPage, onRemoveAll, onSignOutRemove, Modifier.fillMaxWidth())
            }.first().measure(constraints.copy(minWidth = 0, minHeight = 0))
            val height = tabPlace.height + iconPlace.height
            layout(constraints.maxWidth, height) {
                tabPlace.place(0, 0)
                iconPlace.place(0, tabPlace.height)
            }
        }
    }
}

@Composable
private fun TabStrip(page: Int, onPage: (Int) -> Unit, modifier: Modifier = Modifier) {
    Row(
        modifier
            .horizontalScroll(rememberScrollState())
            .padding(start = 8.dp, end = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Segment("Saved", page == 0) { onPage(0) }
        Segment("Recent", page == 1) { onPage(1) }
        Segment("Discovered", page == 2) { onPage(2) }
    }
}

@Composable
private fun IconStrip(
    view: SavedView,
    onView: (SavedView) -> Unit,
    searchOpen: Boolean,
    onSearch: () -> Unit,
    menu: Boolean,
    onMenu: (Boolean) -> Unit,
    sort: SavedSort,
    filter: SavedFilter,
    onSort: (SavedSort) -> Unit,
    onFilter: (SavedFilter) -> Unit,
    savedPage: Boolean,
    onRemoveAll: () -> Unit,
    onSignOutRemove: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.End) {
        ViewButton(PjIcons.ViewList, view == SavedView.List, "List") { onView(SavedView.List) }
        ViewButton(PjIcons.ViewSmall, view == SavedView.Small, "Small tiles") { onView(SavedView.Small) }
        ViewButton(PjIcons.ViewBig, view == SavedView.Big, "Big tiles") { onView(SavedView.Big) }
        if (!searchOpen) {
            HeaderIcon(PjIcons.Search, "Search") { onSearch() }
        }
        Box {
            HeaderIcon(PjIcons.Filter, "Sort and filter") { onMenu(true) }
            DropdownMenu(expanded = menu, onDismissRequest = { onMenu(false) }) {
                Text(
                    "Sort",
                    modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
                    style = MaterialTheme.typography.labelMedium,
                    color = pjMuted(),
                )
                listOf(
                    SavedSort.Recent to "Recently used",
                    SavedSort.Online to "Online first",
                    SavedSort.Name to "Name",
                ).forEach { (value, label) ->
                    DropdownMenuItem(
                        text = { Text(if (sort == value) "$label  ✓" else label) },
                        onClick = { onMenu(false); onSort(value) },
                    )
                }
                HorizontalDivider()
                Text(
                    "Show",
                    modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
                    style = MaterialTheme.typography.labelMedium,
                    color = pjMuted(),
                )
                listOf(
                    SavedFilter.All to "All",
                    SavedFilter.Online to "Online",
                    SavedFilter.Offline to "Offline",
                ).forEach { (value, label) ->
                    DropdownMenuItem(
                        text = { Text(if (filter == value) "$label  ✓" else label) },
                        onClick = { onMenu(false); onFilter(value) },
                    )
                }
                if (savedPage) {
                    HorizontalDivider()
                    DropdownMenuItem(
                        text = { Text("Remove all saved computers from this computer") },
                        onClick = { onMenu(false); onRemoveAll() },
                    )
                    DropdownMenuItem(
                        text = { Text("Sign out and remove everything from this computer") },
                        onClick = { onMenu(false); onSignOutRemove() },
                    )
                }
            }
        }
    }
}

@Composable
internal fun LocalSavedClearDialog(
    title: String,
    count: Int,
    detail: String,
    confirm: String,
    onDismiss: () -> Unit,
    onConfirm: () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(savedComputerCountLine(count))
                Text(detail)
            }
        },
        confirmButton = { TextButton(onClick = onConfirm) { Text(confirm) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun Segment(label: String, selected: Boolean, onClick: () -> Unit) {
    Surface(
        onClick = onClick,
        shape = RoundedCornerShape(8.dp),
        color = if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent,
    ) {
        Text(
            label,
            modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp),
            maxLines = 1,
            softWrap = false,
            overflow = TextOverflow.Visible,
            style = MaterialTheme.typography.labelLarge,
            color = if (selected) MaterialTheme.colorScheme.onSecondaryContainer else MaterialTheme.colorScheme.onSurface,
        )
    }
}

@Composable
private fun ViewButton(icon: ImageVector, selected: Boolean, description: String, onClick: () -> Unit) {
    Box(
        Modifier
            .size(32.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(if (selected) MaterialTheme.colorScheme.primary.copy(alpha = 0.16f) else Color.Transparent)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            icon,
            contentDescription = description,
            modifier = Modifier.size(20.dp),
            tint = if (selected) MaterialTheme.colorScheme.primary else pjMuted(),
        )
    }
}

@Composable
private fun HeaderIcon(icon: ImageVector, description: String, onClick: () -> Unit) {
    Box(
        Modifier
            .size(40.dp)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = description, modifier = Modifier.size(22.dp), tint = MaterialTheme.colorScheme.onSurface)
    }
}
