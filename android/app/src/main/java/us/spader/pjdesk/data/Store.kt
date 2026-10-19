package us.spader.pjdesk.data

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import us.spader.pjdesk.core.SavedComputer
import java.io.File

/**
 * App-private preferences. The permanent password is encrypted with an
 * Android Keystore key. SPAKE2 needs that secret on the host to finish the
 * handshake, so a one-way verifier cannot replace it. The ciphertext is not
 * backed up (`allowBackup` is false).
 */
class Store(context: Context) {
    private val dir = context.filesDir
    private val prefs = context.getSharedPreferences("pjdesk", Context.MODE_PRIVATE)
    private val secret = PermanentSecret(context.applicationContext)
    private val savedFile = File(dir, "saved.json")

    init {
        movePlainPermanentIntoKeystore()
    }

    var onboarded: Boolean
        get() = prefs.getBoolean("onboarded", false)
        set(value) { prefs.edit().putBoolean("onboarded", value).apply() }

    /** The screen-share prompt was completed once. Android 14+ still asks again for each session. */
    var captureTried: Boolean
        get() = prefs.getBoolean("captureTried", false)
        set(value) { prefs.edit().putBoolean("captureTried", value).apply() }

    var theme: String
        get() = prefs.getString("theme", "system") ?: "system"
        set(value) { prefs.edit().putString("theme", value).apply() }

    /** Decrypted permanent password, or empty when none is saved. */
    val permanent: String
        get() = secret.read()

    /**
     * Writes the permanent password and waits until the ciphertext is on disk.
     * An empty value removes it. Returns false when it could not be stored.
     */
    fun savePermanent(password: String): Boolean = secret.write(password)

    var approval: String
        get() = prefs.getString("approval", "prompt") ?: "prompt"
        set(value) { prefs.edit().putString("approval", value).commit() }

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

    /** Older builds kept this secret in the plain preference file. Move it once, then delete the plain copy. */
    private fun movePlainPermanentIntoKeystore() {
        if (!prefs.contains(PLAIN_PERMANENT)) return
        val old = prefs.getString(PLAIN_PERMANENT, "").orEmpty()
        val moved = old.isEmpty() || permanent.isNotEmpty() || savePermanent(old)
        if (moved) prefs.edit().remove(PLAIN_PERMANENT).commit()
    }

    private companion object {
        const val PLAIN_PERMANENT = "permanent"
    }
}
