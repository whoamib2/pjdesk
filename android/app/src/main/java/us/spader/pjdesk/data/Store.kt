package us.spader.pjdesk.data

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.isIdLabel
import us.spader.pjdesk.core.nickname
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
                    val id = o.optString("id")
                    val rawName = o.optString("name")
                    val hasHostname = o.has("hostname")
                    val hostname = if (hasHostname) {
                        o.optString("hostname")
                    } else if (isIdLabel(rawName, id)) {
                        ""
                    } else {
                        rawName
                    }
                    val name = if (hasHostname) rawName else ""
                    add(
                        SavedComputer(
                            id = id,
                            name = name,
                            osFamily = o.optString("osFamily"),
                            osVersion = o.optString("osVersion"),
                            passwordSaved = computerPassword(id).isNotEmpty(),
                            hostname = hostname,
                            consoleUser = o.optString("consoleUser"),
                            lastSeen = o.optLong("lastSeen"),
                            online = o.optBoolean("online"),
                        ),
                    )
                }
            }.filter { it.id.length == 9 }
        }.getOrDefault(emptyList())
    }

    fun remember(computer: SavedComputer) {
        mergeCloud(listOf(computer))
    }

    /**
     * Fills the local Saved list from the account.
     * On first sign-in (`preferAccount`) the account nickname wins a conflict.
     * A later refresh leaves a nickname typed on this phone. Passwords stay here.
     */
    fun mergeCloud(remote: List<SavedComputer>, preferAccount: Boolean = false) {
        val local = saved().toMutableList()
        for (incoming in remote) {
            if (incoming.id.length != 9) continue
            val index = local.indexOfFirst { it.id == incoming.id }
            if (index < 0) {
                local.add(0, incoming.copy(name = incoming.nickname(), passwordSaved = false))
                continue
            }
            val cur = local[index]
            val accountName = incoming.nickname()
            val name = if (preferAccount && accountName.isNotBlank()) accountName else cur.nickname().ifBlank { accountName }
            local[index] = cur.copy(
                name = name,
                hostname = incoming.hostname.ifBlank { cur.hostname },
                osFamily = incoming.osFamily.ifBlank { cur.osFamily },
                osVersion = incoming.osVersion.ifBlank { cur.osVersion },
                consoleUser = incoming.consoleUser.ifBlank { cur.consoleUser },
                lastSeen = maxOf(cur.lastSeen, incoming.lastSeen),
                online = if (incoming.lastSeen > 0 || incoming.online) incoming.online else cur.online,
            )
        }
        writeSaved(local)
    }

    /** Hostname and OS from a session. A nickname already set on this phone is left as it is. */
    fun noteRemote(id: String, hostname: String, osFamily: String, osVersion: String) {
        if (id.length != 9) return
        val host = hostname.trim().let { if (isIdLabel(it, id)) "" else it }
        val current = saved()
        val prev = current.find { it.id == id }
        val next = if (prev == null) {
            SavedComputer(id = id, hostname = host, osFamily = osFamily, osVersion = osVersion)
        } else {
            prev.copy(
                hostname = host.ifBlank { prev.hostname },
                osFamily = osFamily.ifBlank { prev.osFamily },
                osVersion = osVersion.ifBlank { prev.osVersion },
            )
        }
        writeSaved(listOf(next) + current.filterNot { it.id == id })
    }

    fun rename(id: String, nickname: String) {
        val current = saved()
        val prev = current.find { it.id == id } ?: SavedComputer(id = id)
        val next = prev.copy(name = nickname.trim())
        writeSaved(listOf(next) + current.filterNot { it.id == id })
    }

    private fun writeSaved(list: List<SavedComputer>) {
        val array = JSONArray()
        list.take(200).forEach { c ->
            array.put(
                JSONObject()
                    .put("id", c.id)
                    .put("name", c.nickname())
                    .put("hostname", c.hostname)
                    .put("osFamily", c.osFamily)
                    .put("osVersion", c.osVersion)
                    .put("consoleUser", c.consoleUser)
                    .put("lastSeen", c.lastSeen)
                    .put("online", c.online),
            )
        }
        savedFile.writeText(array.toString())
    }

    var networkEmail: String
        get() = prefs.getString("networkEmail", "") ?: ""
        set(value) { prefs.edit().putString("networkEmail", value).apply() }

    /** The encrypted network key last applied here. The passphrase is not stored. */
    var networkKey: String
        get() = prefs.getString("networkKey", "") ?: ""
        set(value) { prefs.edit().putString("networkKey", value).apply() }

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
        writeSaved(saved().filterNot { it.id == id })
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
