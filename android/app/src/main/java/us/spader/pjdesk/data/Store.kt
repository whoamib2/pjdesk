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
    private val refreshBlob = KeystoreBlob(context.applicationContext, "pjdesk-network-refresh", "refresh")
    private val passwordBlob = KeystoreBlob(context.applicationContext, "pjdesk-computer-passwords", "passwords")
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
                            passwordSaved = computerPassword(o.optString("id")).isNotEmpty(),
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

    var networkEmail: String
        get() = prefs.getString("networkEmail", "") ?: ""
        set(value) { prefs.edit().putString("networkEmail", value).apply() }

    var networkDeviceId: String
        get() = prefs.getString("networkDevice", "") ?: ""
        set(value) { prefs.edit().putString("networkDevice", value).apply() }

    fun networkRefresh(): String = refreshBlob.read()

    fun saveNetworkRefresh(token: String): Boolean = refreshBlob.write(token)

    /** A password for one remote computer. It stays in the Keystore on this phone. */
    fun computerPassword(id: String): String {
        val raw = passwordBlob.read()
        if (raw.isEmpty()) return ""
        return runCatching { JSONObject(raw).optString(id) }.getOrDefault("")
    }

    fun saveComputerPassword(id: String, password: String): Boolean {
        val obj = runCatching { JSONObject(passwordBlob.read().ifEmpty { "{}" }) }.getOrDefault(JSONObject())
        if (password.isEmpty()) obj.remove(id) else obj.put(id, password)
        return passwordBlob.write(obj.toString())
    }

    fun forget(id: String) {
        saveComputerPassword(id, "")
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
