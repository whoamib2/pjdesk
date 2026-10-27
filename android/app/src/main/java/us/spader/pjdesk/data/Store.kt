package us.spader.pjdesk.data

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import us.spader.pjdesk.core.RemoteOptions
import us.spader.pjdesk.core.SavedComputer
import us.spader.pjdesk.core.VaultEntry
import us.spader.pjdesk.core.isIdLabel
import us.spader.pjdesk.core.nickname
import java.io.File
import java.util.Base64

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
    private val accessKeyBlob = KeystoreBlob(context.applicationContext, "pjdesk-server-access-key", "access-key")
    private val passwordBlob = KeystoreBlob(context.applicationContext, "pjdesk-computer-passwords", "passwords")
    private val savedFile = File(dir, "saved.json")
    private val snapshotFile = File(dir, "saved-before-temp.json")
    private val markerFile = File(dir, "temp-session.json")

    init {
        movePlainPermanentIntoKeystore()
        decideSavedSyncOnLoad()
        if (markerFile.exists()) endTemporaryVisit()
    }

    /**
     * `yes` downloads the account's saved computers. `no` leaves this phone's
     * list alone. Empty is a phone from before the choice.
     */
    var savedSyncChoice: String
        get() = prefs.getString("savedSync", "") ?: ""
        set(value) { prefs.edit().putString("savedSync", value).commit() }

    fun syncsSavedComputers(): Boolean = savedSyncChoice == "yes"

    /** An update keeps a list that is already on this phone. It does not wipe it. */
    fun decideSavedSyncOnLoad() {
        val next = us.spader.pjdesk.core.savedSyncChoiceOnUpdate(savedSyncChoice, saved().size)
        if (next != savedSyncChoice) savedSyncChoice = next
    }

    /** Server access key. Sealed in the Android keystore. Not written to pjdesk.json. */
    var accessKey: String
        get() = accessKeyBlob.read()
        set(value) { accessKeyBlob.write(value.trim()) }

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

    /**
     * The name this phone reports to other computers. Empty means the Android
     * device name (or the model) is used. Survives an app update; uninstall clears it.
     */
    var computerName: String
        get() = prefs.getString("computerName", "") ?: ""
        set(value) {
            val cleaned = value.trim().filter { !it.isISOControl() }.take(64)
            prefs.edit().putString("computerName", cleaned).apply()
        }

    /** Trackpad multiplier from 0.25 to 2. 0.6 is slower than the old 1× speed. */
    var pointerSpeed: Float
        get() = prefs.getFloat("pointerSpeed", 0.6f).coerceIn(0.25f, 2f)
        set(value) { prefs.edit().putFloat("pointerSpeed", value.coerceIn(0.25f, 2f)).apply() }

    /** Off by default, so 0.6× stays linear. */
    var pointerAccel: Boolean
        get() = prefs.getBoolean("pointerAccel", false)
        set(value) { prefs.edit().putBoolean("pointerAccel", value).apply() }

    /**
     * Minutes with no input before a controller session ends.
     * 0 is off. The same value applies in the foreground, in the background, and with the screen off.
     */
    var sessionIdleMin: Int
        get() {
            val stored = prefs.getInt("sessionIdleMin", 1)
            return if (stored == 0 || stored == 1 || stored == 5 || stored == 15) stored else 1
        }
        set(value) {
            val cleaned = if (value == 0 || value == 1 || value == 5 || value == 15) value else 1
            prefs.edit().putInt("sessionIdleMin", cleaned).apply()
        }

    /** Defaults for every computer. Pointer speed, acceleration, and idle minutes stay on their own keys. */
    fun remoteGlobal(): RemoteOptions {
        val parsed = RemoteOptions.fromJson(prefs.getString("remoteOptions", null))
        return parsed.copy(
            pointerSpeed = pointerSpeed,
            pointerAccel = pointerAccel,
            sessionIdleMin = sessionIdleMin,
        ).cleaned()
    }

    fun saveRemoteGlobal(options: RemoteOptions) {
        val cleaned = options.cleaned()
        pointerSpeed = cleaned.pointerSpeed
        pointerAccel = cleaned.pointerAccel
        sessionIdleMin = cleaned.sessionIdleMin
        prefs.edit().putString("remoteOptions", cleaned.toJson().toString()).apply()
    }

    /** A full copy for one 9-digit id, or null when that computer uses the defaults. */
    fun remoteOverride(id: String): RemoteOptions? {
        if (id.length != 9) return null
        val all = runCatching { JSONObject(prefs.getString("remoteOverrides", "{}") ?: "{}") }.getOrElse { return null }
        val one = all.optJSONObject(id) ?: return null
        return RemoteOptions.fromJson(one.toString())
    }

    fun saveRemoteOverride(id: String, options: RemoteOptions?) {
        if (id.length != 9) return
        val all = runCatching { JSONObject(prefs.getString("remoteOverrides", "{}") ?: "{}") }.getOrElse { JSONObject() }
        if (options == null) all.remove(id) else all.put(id, options.cleaned().toJson())
        prefs.edit().putString("remoteOverrides", all.toString()).apply()
    }

    /** Decrypted permanent password, or empty when none is saved. */
    val permanent: String
        get() = secret.read()

    /**
     * Writes the permanent password and waits until the ciphertext is on disk.
     * An empty value removes it. Returns false when it could not be stored.
     */
    fun savePermanent(password: String): Boolean = secret.write(password)

    /**
     * Seconds when this phone last published its permanent password into the account vault.
     * A newer vault entry for this phone's own id can replace the local password.
     */
    var permanentPublished: Long
        get() = prefs.getLong("permanentPublished", 0L)
        set(value) { prefs.edit().putLong("permanentPublished", value).commit() }

    var approval: String
        get() = prefs.getString("approval", "prompt") ?: "prompt"
        set(value) { prefs.edit().putString("approval", value).commit() }

    /** Unattended access (accept automatically). Off until the person turns it on. */
    var unattendedAccess: Boolean
        get() = prefs.getBoolean("unattendedAccess", false)
        set(value) { prefs.edit().putBoolean("unattendedAccess", value).commit() }

    /**
     * 0.6.17 stored "Use the permanent password" separately from the switch.
     * Copy that choice onto the switch once. Later launches leave the switch
     * alone, including when the person turns it off.
     * Returns true when this call turned the switch on.
     */
    fun migrateUnattendedAccess(): Boolean {
        if (prefs.getBoolean("unattendedMigrated", false)) return false
        val turnOn = approval == "unattended" && !unattendedAccess
        if (turnOn) unattendedAccess = true
        prefs.edit().putBoolean("unattendedMigrated", true).commit()
        return turnOn
    }

    fun trustedAccounts(): List<String> = readTrusted("trustedAccounts")

    fun trustedComputers(): List<String> = readTrusted("trustedComputers")

    fun allowAccount(email: String) {
        val cleaned = email.trim().filter { !it.isISOControl() }.take(80)
        if (cleaned.isEmpty() || !cleaned.contains("@")) return
        val current = readTrusted("trustedAccounts")
        if (current.any { it.equals(cleaned, ignoreCase = true) }) return
        writeTrusted("trustedAccounts", current + cleaned)
    }

    fun allowComputer(name: String) {
        val cleaned = name.trim().filter { !it.isISOControl() }.take(64)
        if (cleaned.isEmpty()) return
        val current = readTrusted("trustedComputers")
        if (current.any { it.equals(cleaned, ignoreCase = true) }) return
        writeTrusted("trustedComputers", current + cleaned)
    }

    /** `kind` is "account" or "computer". */
    fun forgetTrusted(kind: String, value: String) {
        val key = if (kind == "account") "trustedAccounts" else "trustedComputers"
        writeTrusted(key, readTrusted(key).filterNot { it.equals(value, ignoreCase = true) })
    }

    private fun readTrusted(key: String): List<String> {
        val raw = prefs.getString(key, "[]") ?: "[]"
        val array = runCatching { JSONArray(raw) }.getOrElse { return emptyList() }
        return buildList {
            for (i in 0 until array.length()) {
                val item = array.optString(i).trim()
                if (item.isNotEmpty() && none { it.equals(item, ignoreCase = true) }) add(item)
            }
        }
    }

    private fun writeTrusted(key: String, values: List<String>) {
        val array = JSONArray()
        values.forEach { array.put(it) }
        prefs.edit().putString(key, array.toString()).commit()
    }

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
                            lastSeen = us.spader.pjdesk.core.seenSeconds(o.optLong("lastSeen")),
                            online = o.optBoolean("online"),
                            displayName = o.optString("displayName"),
                            appVersion = o.optString("appVersion"),
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
     * A later refresh leaves a nickname typed on this phone. Passwords are a separate sealed vault.
     */
    fun mergeCloud(remote: List<SavedComputer>, preferAccount: Boolean = false) {
        if (!us.spader.pjdesk.core.mergeSavedFromAccount(localSavedCleared, preferAccount)) return
        if (preferAccount) localSavedCleared = false
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
                lastSeen = maxOf(
                    us.spader.pjdesk.core.seenSeconds(cur.lastSeen),
                    us.spader.pjdesk.core.seenSeconds(incoming.lastSeen),
                ),
                online = if (us.spader.pjdesk.core.seenSeconds(incoming.lastSeen) > 0L || incoming.online) {
                    incoming.online
                } else {
                    cur.online
                },
                displayName = incoming.displayName.ifBlank { cur.displayName },
                appVersion = incoming.appVersion.ifBlank { cur.appVersion },
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
                    .put("online", c.online)
                    .put("displayName", c.displayName)
                    .put("appVersion", c.appVersion),
            )
        }
        savedFile.writeText(array.toString())
    }

    var networkEmail: String
        get() = prefs.getString("networkEmail", "") ?: ""
        set(value) { prefs.edit().putString("networkEmail", value).apply() }

    /** Joined with a code or a key. The account email may be known. The password was not typed here. */
    var joinedWithoutPassword: Boolean
        get() = prefs.getBoolean("joinedWithoutPassword", false)
        set(value) { prefs.edit().putBoolean("joinedWithoutPassword", value).apply() }

    /** The encrypted network key last applied here. The passphrase is not stored. */
    var networkKey: String
        get() = prefs.getString("networkKey", "") ?: ""
        set(value) { prefs.edit().putString("networkKey", value).apply() }

    var networkDeviceId: String
        get() = prefs.getString("networkDevice", "") ?: ""
        set(value) { prefs.edit().putString("networkDevice", value).apply() }

    /** Last account server, so Connect can proceed if the engine has not applied it yet. */
    var accountServer: String
        get() = prefs.getString("accountServer", "") ?: ""
        set(value) { prefs.edit().putString("accountServer", value).apply() }

    /** Standby server from the account. The phone registers here when the primary stops answering. */
    var accountStandby: String
        get() = prefs.getString("accountStandby", "") ?: ""
        set(value) { prefs.edit().putString("accountStandby", value).apply() }

    var accountFingerprint: String
        get() = prefs.getString("accountFingerprint", "") ?: ""
        set(value) { prefs.edit().putString("accountFingerprint", value).apply() }

    /** Comma-separated private addresses of the active server, from the account. */
    var accountLan: String
        get() = prefs.getString("accountLan", "") ?: ""
        set(value) { prefs.edit().putString("accountLan", value).apply() }

    var accountPublicIp: String
        get() = prefs.getString("accountPublicIp", "") ?: ""
        set(value) { prefs.edit().putString("accountPublicIp", value).apply() }

    var myPublicIp: String
        get() = prefs.getString("myPublicIp", "") ?: ""
        set(value) { prefs.edit().putString("myPublicIp", value).apply() }

    /** LAN address that finished the handshake on this network, keyed by gateway or Wi-Fi name. */
    fun lanPath(networkKey: String): String {
        if (networkKey.isBlank()) return ""
        return lanPaths().optString(networkKey)
    }

    fun rememberLanPath(networkKey: String, address: String) {
        if (networkKey.isBlank() || address.isBlank()) return
        val map = lanPaths()
        map.put(networkKey, address)
        prefs.edit().putString("lanPaths", map.toString()).apply()
    }

    private fun lanPaths(): org.json.JSONObject {
        val raw = prefs.getString("lanPaths", "") ?: ""
        return runCatching { org.json.JSONObject(raw) }.getOrDefault(org.json.JSONObject())
    }

    var pinWrap: String
        get() = prefs.getString("pinWrap", "") ?: ""
        set(value) { prefs.edit().putString("pinWrap", value).commit() }

    var pinSalt: String
        get() = prefs.getString("pinSalt", "") ?: ""
        set(value) { prefs.edit().putString("pinSalt", value).commit() }

    var bioWrap: String
        get() = prefs.getString("bioWrap", "") ?: ""
        set(value) { prefs.edit().putString("bioWrap", value).commit() }

    var pinFails: Int
        get() = prefs.getInt("pinFails", 0)
        set(value) { prefs.edit().putInt("pinFails", value).commit() }

    var biometricsOn: Boolean
        get() = prefs.getBoolean("biometricsOn", false)
        set(value) { prefs.edit().putBoolean("biometricsOn", value).commit() }

    var autoLockSeconds: Int
        get() {
            val value = prefs.getInt("autoLockSeconds", 300)
            return if (value == 60 || value == 120 || value == 300 || value == 900 || value == 1800) value else 300
        }
        set(value) {
            val kept = if (value == 60 || value == 120 || value == 300 || value == 900 || value == 1800) value else 300
            prefs.edit().putInt("autoLockSeconds", kept).apply()
        }

    var accountPinSalt: String
        get() = prefs.getString("accountPinSalt", "") ?: ""
        set(value) { prefs.edit().putString("accountPinSalt", value).commit() }

    var accountPinHash: String
        get() = prefs.getString("accountPinHash", "") ?: ""
        set(value) { prefs.edit().putString("accountPinHash", value).commit() }

    var accountPinIters: Int
        get() = prefs.getInt("accountPinIters", 0)
        set(value) { prefs.edit().putInt("accountPinIters", value).commit() }

    var accountPinUpdated: Long
        get() = prefs.getLong("accountPinUpdated", 0L)
        set(value) { prefs.edit().putLong("accountPinUpdated", value).commit() }

    var accountPinIdle: Int
        get() = prefs.getInt("accountPinIdle", 300)
        set(value) { prefs.edit().putInt("accountPinIdle", value).commit() }

    /** Digit count stored with the account PIN. 0 means it was not recorded. Not shown. */
    var accountPinLen: Int
        get() = prefs.getInt("accountPinLen", 0)
        set(value) { prefs.edit().putInt("accountPinLen", value).commit() }

    /** Digit count of a PIN that is only on this phone. 0 means it was not recorded. Not shown. */
    var localPinLen: Int
        get() = prefs.getInt("localPinLen", 0)
        set(value) { prefs.edit().putInt("localPinLen", value).commit() }

    var appLockSealed: String
        get() = prefs.getString("appLockSealed", "") ?: ""
        set(value) { prefs.edit().putString("appLockSealed", value).commit() }

    var appLockUpdated: Long
        get() = prefs.getLong("appLockUpdated", 0L)
        set(value) { prefs.edit().putLong("appLockUpdated", value).commit() }

    /** The PIN is on this phone and has not reached the account yet. */
    var appLockPending: Boolean
        get() = prefs.getBoolean("appLockPending", false)
        set(value) { prefs.edit().putBoolean("appLockPending", value).commit() }

    var pinLockedUntil: Long
        get() = prefs.getLong("pinLockedUntil", 0L)
        set(value) { prefs.edit().putLong("pinLockedUntil", value).commit() }

    var bioForUpdated: Long
        get() = prefs.getLong("bioForUpdated", 0L)
        set(value) { prefs.edit().putLong("bioForUpdated", value).commit() }

    var accountPinEmail: String
        get() = prefs.getString("accountPinEmail", "") ?: ""
        set(value) { prefs.edit().putString("accountPinEmail", value).commit() }

    fun hasAccountPin(): Boolean {
        val stored = accountPinEmail.trim().lowercase()
        val email = networkEmail.trim().lowercase().ifBlank {
            if (joinedWithoutPassword) stored else ""
        }
        return email.isNotEmpty()
            && email == stored
            && accountPinHash.length == 64
            && accountPinSalt.isNotBlank()
            && accountPinIters > 0
    }

    fun saveAccountPin(email: String, verifier: us.spader.pjdesk.core.AccountPin.Verifier, sealed: String, updated: Long, pending: Boolean = false) {
        val idle = when (verifier.idleSec) {
            60, 900, 1800 -> verifier.idleSec
            else -> 300
        }
        prefs.edit()
            .putString("accountPinEmail", email.trim().lowercase())
            .putString("accountPinSalt", verifier.salt)
            .putString("accountPinHash", verifier.hash)
            .putInt("accountPinIters", verifier.iterations)
            .putLong("accountPinUpdated", verifier.updated)
            .putInt("accountPinIdle", verifier.idleSec)
            .putInt("accountPinLen", verifier.len)
            .putString("appLockSealed", sealed)
            .putLong("appLockUpdated", updated)
            .putBoolean("appLockPending", pending)
            .putInt("pinFails", 0)
            .putLong("pinLockedUntil", 0L)
            .putInt("autoLockSeconds", idle)
            .commit()
    }

    /** A sealed PIN arrived before this phone could open it. The hash stays as it was. */
    fun notePendingAppLock(sealed: String, updated: Long) {
        prefs.edit()
            .putString("appLockSealed", sealed)
            .putLong("appLockUpdated", updated)
            .commit()
    }

    fun clearAccountPin() {
        prefs.edit()
            .putString("accountPinEmail", "")
            .putString("accountPinSalt", "")
            .putString("accountPinHash", "")
            .putInt("accountPinIters", 0)
            .putLong("accountPinUpdated", 0L)
            .putString("appLockSealed", "")
            .putLong("appLockUpdated", 0L)
            .putBoolean("appLockPending", false)
            .putInt("accountPinLen", 0)
            .putLong("bioForUpdated", 0L)
            .putInt("pinFails", 0)
            .putLong("pinLockedUntil", 0L)
            .commit()
    }

    fun hasPin(): Boolean = pinWrap.isNotEmpty() && pinSaltBytes()?.size == 16

    fun pinSaltBytes(): ByteArray? = runCatching { Base64.getDecoder().decode(pinSalt) }.getOrNull()

    fun savePinSalt(salt: ByteArray) {
        pinSalt = Base64.getEncoder().encodeToString(salt)
    }

    fun clearPinMaterial() {
        prefs.edit()
            .putString("pinWrap", "")
            .putString("pinSalt", "")
            .putString("bioWrap", "")
            .putInt("localPinLen", 0)
            .putInt("pinFails", 0)
            .putBoolean("biometricsOn", false)
            .commit()
    }

    /** Minutes with no PJDesk activity before a temporary sign-in ends. */
    var tempIdleMinutes: Int
        get() = (prefs.getInt("tempIdleMinutes", 30)).coerceIn(1, 24 * 60)
        set(value) { prefs.edit().putInt("tempIdleMinutes", value.coerceIn(1, 24 * 60)).apply() }

    fun temporaryActive(): Boolean = markerFile.exists()

    /**
     * The saved list on this phone was cleared. Account refreshes must not
     * put it back. The next explicit sign-in clears this.
     */
    var localSavedCleared: Boolean
        get() = prefs.getBoolean("localSavedCleared", false)
        set(value) { prefs.edit().putBoolean("localSavedCleared", value).commit() }

    /** The next sign-in may fill the saved list from the account again. */
    fun allowSavedMerge() {
        localSavedCleared = false
    }

    /**
     * Drops the saved list, cached passwords, recents, and saved LAN addresses
     * on this phone. The account is not changed.
     */
    fun clearLocalSaved() {
        writeSaved(emptyList())
        passwordBlob.write("{}")
        prefs.edit()
            .putString("passwordTimes", "{}")
            .putString("recent", "{}")
            .putString("lanPaths", "{}")
            .putBoolean("localSavedCleared", true)
            .commit()
    }

    /** Remembers the saved list from before a temporary sign-in. */
    fun beginTemporaryVisit() {
        if (savedFile.exists()) savedFile.copyTo(snapshotFile, overwrite = true) else snapshotFile.writeText("[]")
        markerFile.writeText("""{"passwords":[]}""")
    }

    fun noteTemporaryPassword(id: String) {
        if (!markerFile.exists() || id.length != 9) return
        val obj = runCatching { JSONObject(markerFile.readText()) }.getOrDefault(JSONObject())
        val arr = obj.optJSONArray("passwords") ?: JSONArray()
        for (i in 0 until arr.length()) if (arr.optString(i) == id) return
        arr.put(id)
        obj.put("passwords", arr)
        markerFile.writeText(obj.toString())
    }

    /**
     * Ends a temporary sign-in. The saved list, cached passwords, and recents
     * on this phone are cleared. The account on pjdesk.com is not deleted.
     */
    fun endTemporaryVisit() {
        if (!markerFile.exists() && !snapshotFile.exists()) return
        clearLocalSaved()
        networkEmail = ""
        joinedWithoutPassword = false
        saveNetworkRefresh("")
        snapshotFile.delete()
        markerFile.delete()
    }

    fun networkRefresh(): String = refreshBlob.read()

    fun saveNetworkRefresh(token: String): Boolean = refreshBlob.write(token)

    /** A password for one remote computer, sealed on this phone and synced as ciphertext. */
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

    /** Saves a password or a deletion and stamps the time the account vault uses. */
    fun stampPassword(id: String, password: String): Boolean {
        if (id.length != 9) return false
        val ok = saveComputerPassword(id, password)
        val times = readTimes()
        times.put(id, System.currentTimeMillis() / 1000)
        writeTimes(times)
        return ok
    }

    fun vaultEntries(): List<VaultEntry> {
        val passwords = readPasswords()
        val times = readTimes()
        val ids = linkedSetOf<String>()
        val passwordKeys = passwords.keys()
        while (passwordKeys.hasNext()) {
            val id = passwordKeys.next()
            if (id.length == 9) ids.add(id)
        }
        val timeKeys = times.keys()
        while (timeKeys.hasNext()) {
            val id = timeKeys.next()
            if (id.length == 9) ids.add(id)
        }
        return ids.map { id -> VaultEntry(id, passwords.optString(id), times.optLong(id)) }
            .filter { it.password.isNotEmpty() || it.updated > 0 }
    }

    fun applyVault(entries: List<VaultEntry>) {
        val passwords = readPasswords()
        val times = readTimes()
        for (entry in entries) {
            if (entry.id.length != 9) continue
            if (entry.password.isEmpty()) passwords.remove(entry.id) else passwords.put(entry.id, entry.password)
            if (entry.updated > 0) times.put(entry.id, entry.updated) else times.remove(entry.id)
        }
        passwordBlob.write(passwords.toString())
        writeTimes(times)
    }

    fun forget(id: String) {
        stampPassword(id, "")
        writeSaved(saved().filterNot { it.id == id })
    }

    private fun readPasswords(): JSONObject {
        val raw = passwordBlob.read()
        if (raw.isEmpty()) return JSONObject()
        return runCatching { JSONObject(raw) }.getOrDefault(JSONObject())
    }

    private fun readTimes(): JSONObject {
        val raw = prefs.getString("passwordTimes", "{}") ?: "{}"
        return runCatching { JSONObject(raw) }.getOrDefault(JSONObject())
    }

    private fun writeTimes(obj: JSONObject) {
        prefs.edit().putString("passwordTimes", obj.toString()).commit()
    }

    /** Computers this phone has connected to, newest first. Stays on this phone. */
    fun touchRecent(id: String) {
        val digits = id.filter { it.isDigit() }
        if (digits.length != 9) return
        val obj = readRecent()
        obj.put(digits, System.currentTimeMillis())
        val ranked = buildList {
            val keys = obj.keys()
            while (keys.hasNext()) {
                val key = keys.next()
                if (key.length == 9) add(key to obj.optLong(key))
            }
        }.sortedByDescending { it.second }.take(40)
        val kept = JSONObject()
        ranked.forEach { (key, whenMs) -> kept.put(key, whenMs) }
        prefs.edit().putString("recent", kept.toString()).apply()
    }

    fun recentIds(): List<String> {
        val obj = readRecent()
        return buildList {
            val keys = obj.keys()
            while (keys.hasNext()) {
                val key = keys.next()
                if (key.length == 9) add(key to obj.optLong(key))
            }
        }.sortedByDescending { it.second }.map { it.first }
    }

    private fun readRecent(): JSONObject {
        val raw = prefs.getString("recent", "{}") ?: "{}"
        return runCatching { JSONObject(raw) }.getOrDefault(JSONObject())
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
