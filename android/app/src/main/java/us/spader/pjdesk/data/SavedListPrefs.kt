package us.spader.pjdesk.data

import android.content.Context
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import us.spader.pjdesk.core.SavedFilter
import us.spader.pjdesk.core.SavedSort
import us.spader.pjdesk.core.SavedView

private val Context.savedListDataStore by preferencesDataStore(name = "pjdesk_saved_list")

/** Sort and filter for Saved computers. Remembered on this phone. */
class SavedListPrefs(private val context: Context) {
    private val sortKey = stringPreferencesKey("sort")
    private val filterKey = stringPreferencesKey("filter")
    private val viewKey = stringPreferencesKey("view")

    val sort: Flow<SavedSort> = context.savedListDataStore.data.map { prefs ->
        SavedSort.fromStorage(prefs[sortKey])
    }

    val filter: Flow<SavedFilter> = context.savedListDataStore.data.map { prefs ->
        SavedFilter.fromStorage(prefs[filterKey])
    }

    val view: Flow<SavedView> = context.savedListDataStore.data.map { prefs ->
        SavedView.fromStorage(prefs[viewKey])
    }

    suspend fun setSort(sort: SavedSort) {
        context.savedListDataStore.edit { it[sortKey] = sort.storage }
    }

    suspend fun setFilter(filter: SavedFilter) {
        context.savedListDataStore.edit { it[filterKey] = filter.storage }
    }

    suspend fun setView(view: SavedView) {
        context.savedListDataStore.edit { it[viewKey] = view.storage }
    }
}
