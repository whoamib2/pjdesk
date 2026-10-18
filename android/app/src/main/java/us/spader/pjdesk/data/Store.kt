package us.spader.pjdesk.data

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import us.spader.pjdesk.core.SavedComputer
import java.io.File

/**
 * App-private preferences. Passwords stay in this directory, which is not
 * backed up (`allowBackup` is false) and is not a world-readable store.
 */
class Store(context: Context) {
    private val dir = context.filesDir
    private val prefs = context.getSharedPreferences("pjdesk", Context.MODE_PRIVATE)
    private val savedFile = File(dir, "saved.json")

    var onboarded: Boolean
        get() = prefs.getBoolean("onboarded", false)
        set(value) { prefs.edit().putBoolean("onboarded", value).apply() }

    var theme: String
        get() = prefs.getString("theme", "system") ?: "system"
        set(value) { prefs.edit().putString("theme", value).apply() }

    var permanent: String
        get() = prefs.getString("permanent", "") ?: ""
        set(value) { prefs.edit().putString("permanent", value).apply() }

    var approval: String
        get() = prefs.getString("approval", "prompt") ?: "prompt"
        set(value) { prefs.edit().putString("approval", value).apply() }

    fun saved(): List<SavedComputer> {
        if (!savedFile.exists()) return emptyList()
        return runCatching {
            val array = JSONArray(savedFile.readText())
            buildList {
                for (i in 0 until array.length()) {
                    val o = array.getJSONObject(i)
                    add(
                        SavedComputer(
                            id = o.optString("id"),
                            name = o.optString("name"),
                            osFamily = o.optString("osFamily"),
                            osVersion = o.optString("osVersion"),
                        ),
                    )
                }
            }.filter { it.id.length == 9 }
        }.getOrDefault(emptyList())
    }

    fun remember(computer: SavedComputer) {
        val next = saved().filterNot { it.id == computer.id }.toMutableList()
        next.add(0, computer)
        val array = JSONArray()
        next.take(200).forEach { c ->
            array.put(
                JSONObject()
                    .put("id", c.id)
                    .put("name", c.name)
                    .put("osFamily", c.osFamily)
                    .put("osVersion", c.osVersion),
            )
        }
        savedFile.writeText(array.toString())
    }

    fun forget(id: String) {
        val next = saved().filterNot { it.id == id }
        val array = JSONArray()
        next.forEach { c ->
            array.put(
                JSONObject()
                    .put("id", c.id)
                    .put("name", c.name)
                    .put("osFamily", c.osFamily)
                    .put("osVersion", c.osVersion),
            )
        }
        savedFile.writeText(array.toString())
    }
}
