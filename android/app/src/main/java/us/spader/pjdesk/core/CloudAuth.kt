package us.spader.pjdesk.core

import org.json.JSONArray
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import java.util.concurrent.atomic.AtomicInteger
import javax.crypto.Mac
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.PBEKeySpec
import javax.crypto.spec.SecretKeySpec

/**
 * PJDesk account sign-in. The 600,000-round derivation must run off the UI
 * thread. Only the auth key is sent. The sync key is derived and discarded
 * The password-vault key is kept on the phone and seals saved computer
 * passwords. pjdesk.com stores only that ciphertext.
 */
/**
 * pjdesk.com is used only when no PJDesk server is answering.
 * The wait starts at one minute and stops at 30 minutes.
 */
object CloudGate {
    @Volatile var serverUp: Boolean = false
    @Volatile var accountJson: String = ""
    val accountGen = AtomicInteger(0)
    @Volatile var nextAt: Long = 0
    @Volatile var waitMs: Long = 0

    /** Set by the activity. Runs the account request on a background thread. */
    @Volatile var exchange: ((JSONObject) -> JSONObject?)? = null

    fun allowCloud(): Boolean = !serverUp && System.currentTimeMillis() >= nextAt

    fun noteOk() {
        waitMs = 0
        nextAt = 0
    }

    fun noteFail() {
        val next = if (waitMs <= 0L) 60_000L else (waitMs * 2).coerceAtMost(30L * 60_000)
        waitMs = next
        nextAt = System.currentTimeMillis() + next
    }

    fun noteServer(up: Boolean) {
        serverUp = up
        if (up) noteOk()
    }

    fun noteAccount(json: String) {
        accountJson = json
        accountGen.incrementAndGet()
        if (runCatching { JSONObject(json).optBoolean("ok") }.getOrDefault(false)) noteOk()
    }

    fun request(body: JSONObject): JSONObject? = exchange?.invoke(body)
}

object CloudAuth {
    const val API = "https://pjdesk.com"
    const val NO_SERVER =
        "Your network has no server yet. Sign in to PJDesk Server on your server computer, or set it on pjdesk.com → Account → My network."

    /** The owner of an empty network can publish the server this phone already uses. */
    fun offerCurrentServer(role: String, server: String, fingerprint: String): Boolean =
        role == "owner" && server.isNotBlank() && normalizeFingerprint(fingerprint).length == 64

    fun canonical(email: String, networkId: String, fingerprints: List<String>, deviceId: String, exp: Long): String {
        val fps = fingerprints.map { normalizeFingerprint(it) }.filter { it.isNotEmpty() }.toSortedSet().joinToString(",")
        return "PJN1\n${email.trim().lowercase()}\n${networkId.trim()}\n$fps\n${deviceId.trim()}\n$exp\n"
    }

    fun normalizeFingerprint(text: String): String {
        val raw = text.trim().removePrefix("sha256:").removePrefix("SHA256:")
        return raw.lowercase().filter { it.isDigit() || it in 'a'..'f' }
    }

    fun masterKey(email: String, password: String): ByteArray {
        val salt = MessageDigest.getInstance("SHA-256")
            .digest("pjdesk-cloud-v1\u0000${email.trim().lowercase()}".toByteArray())
        val spec = PBEKeySpec(password.toCharArray(), salt, 600_000, 256)
        return SecretKeyFactory.getInstance("PBKDF2WithHmacSHA256").generateSecret(spec).encoded
    }

    fun authKeyHex(email: String, password: String): String =
        hmac(masterKey(email, password), "pjdesk-cloud-auth").joinToString("") { "%02x".format(it) }

    /** 32 bytes. Never send this. It seals the saved-computer password vault. */
    fun vaultKey(email: String, password: String): ByteArray = hmac(masterKey(email, password), "pjdesk-cloud-password-vault")

    fun fetchPasswordVault(refresh: String, deviceId: String): String {
        val reply = accountOnServer("vault-read", refresh, deviceId)
        return reply.optString("vault")
    }

    fun putPasswordVault(refresh: String, deviceId: String, vault: String) {
        accountOnServer("vault-write", refresh, deviceId, JSONObject().put("vault", vault))
    }

    /** Sealed app-lock blob and the account's updated time. Empty when no PIN is set. */
    fun fetchAppLock(refresh: String, deviceId: String): Pair<String, Long> {
        val reply = accountOnServer("lock-read", refresh, deviceId)
        return reply.optString("app_lock") to reply.optLong("app_lock_updated")
    }

    fun putAppLock(refresh: String, deviceId: String, lock: String): Long {
        val reply = accountOnServer("lock-write", refresh, deviceId, JSONObject().put("app_lock", lock))
        return reply.optLong("app_lock_updated").coerceAtLeast(1)
    }

    private fun accountOnServer(op: String, refresh: String, deviceId: String, extra: JSONObject = JSONObject()): JSONObject {
        if (!CloudGate.serverUp) {
            throw CloudAuthException("No PJDesk server answered. Saved passwords stay on this phone until a server does.")
        }
        val body = JSONObject().put("op", op).put("refresh", refresh).put("device_id", deviceId)
        val keys = extra.keys()
        while (keys.hasNext()) {
            val key = keys.next()
            body.put(key, extra.get(key))
        }
        val reply = CloudGate.request(body)
            ?: throw CloudAuthException("No PJDesk server answered. Saved passwords stay on this phone until a server does.")
        if (!reply.optBoolean("ok")) {
            throw CloudAuthException(reply.optString("message").ifBlank { "Sign in required." })
        }
        return reply
    }

    /** Proves the account password. The short session this creates is dropped. */
    fun confirmPassword(email: String, password: String) {
        val auth = authKeyHex(email, password)
        val local = if (CloudGate.serverUp) {
            CloudGate.request(
                JSONObject().put("op", "verify").put("email", email.trim().lowercase()).put("auth_key", auth),
            )
        } else {
            null
        }
        if (local?.optBoolean("ok") == true) return
        if (local != null) {
            val message = local.optString("message").ifBlank { "Could not sign in." }
            val pause = local.optLong("pause")
            val finalAnswer = message.contains("Wrong email or password")
                || message.contains("Too many sign-in attempts")
                || message.contains("Enter your email")
                || pause > 0
            if (finalAnswer) {
                throw CloudAuthException(message, status = if (pause > 0) 429 else 401, retryAfter = pause.takeIf { it > 0 })
            }
        }
        if (CloudGate.serverUp) throw CloudAuthException("No PJDesk server answered.")
        if (!CloudGate.allowCloud()) throw CloudAuthException("Waiting before asking pjdesk.com again.")
        val login = post(
            "$API/api/login",
            null,
            JSONObject()
                .put("email", email.trim().lowercase())
                .put("auth_key", authKeyHex(email, password))
                .put("device", "PJDesk PIN check"),
        )
        val token = login.optString("token")
        if (token.isNotBlank()) {
            runCatching { post("$API/api/logout", token, JSONObject()) }
        }
        CloudGate.noteOk()
    }

    private fun hmac(key: ByteArray, data: String): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        return mac.doFinal(data.toByteArray())
    }

    fun signIn(email: String, password: String, deviceId: String, deviceName: String): Session =
        signIn(email, password, deviceId, deviceName, forceCloud = false)

    /**
     * Asks pjdesk.com even when a local server is up. Used when that server
     * answered and still has no account copy. Backoff still applies.
     */
    fun signIn(email: String, password: String, deviceId: String, deviceName: String, forceCloud: Boolean): Session {
        if (CloudGate.serverUp && !forceCloud) throw CloudAuthException("The PJDesk server has this account.")
        if (!CloudGate.allowCloud()) throw CloudAuthException("Waiting before asking pjdesk.com again.")
        val auth = authKeyHex(email, password)
        val login = try {
            post(
            "$API/api/login",
            null,
            JSONObject().put("email", email.trim().lowercase()).put("auth_key", auth).put("device", "PJDesk on $deviceName"),
        )
        } catch (e: CloudAuthException) {
            noteTransport(e)
            throw e
        }
        val token = login.getString("token")
        try {
            val reply = post(
                "$API/api/network/token",
                token,
                JSONObject().put("device_id", deviceId).put("device_name", deviceName),
            )
            CloudGate.noteOk()
            return session(email.trim().lowercase(), deviceId, reply)
        } catch (e: CloudAuthException) {
            noteTransport(e)
            val old = e.message?.contains("Set the server address") == true
            if (e.code == "no_server" || old || e.message == NO_SERVER) {
                CloudGate.noteOk()
                val role = if (e.role.isBlank()) "owner" else e.role
                throw CloudAuthException(NO_SERVER, "no_server", role, token)
            }
            throw e
        }
    }

    /** Publishes the server this phone already uses, then finishes sign-in. The token is not stored. */
    fun claimCurrentServer(loginToken: String, address: String, fingerprint: String, deviceId: String, deviceName: String): Session {
        post(
            "$API/api/network/register",
            loginToken,
            JSONObject()
                .put("role", "active")
                .put("address", address)
                .put("fingerprint", fingerprint)
                .put("confirm", true)
                .put("replace", true),
        )
        val reply = post(
            "$API/api/network/token",
            loginToken,
            JSONObject().put("device_id", deviceId).put("device_name", deviceName),
        )
        return session("", deviceId, reply)
    }

    fun hardwareKey(androidId: String): String {
        val raw = androidId.trim()
        if (raw.isEmpty()) return ""
        val digest = java.security.MessageDigest.getInstance("SHA-256").digest(raw.toByteArray(Charsets.UTF_8))
        return digest.take(8).joinToString("") { "%02x".format(it) }
    }

    fun refresh(
        refresh: String,
        deviceId: String,
        computers: JSONArray? = null,
        pjdeskId: String = "",
        hardwareId: String = "",
        hostname: String = "",
    ): Session {
        val body = JSONObject().put("refresh", refresh).put("device_id", deviceId)
        if (computers != null) body.put("computers", computers)
        val id = pjdeskId.filter { it.isDigit() }
        if (id.length == 9) body.put("pjdesk_id", id)
        if (hardwareId.isNotBlank()) body.put("hardware_id", hardwareId.take(64))
        if (hostname.isNotBlank()) body.put("hostname", hostname.take(64))
        if (CloudGate.serverUp) throw CloudAuthException("The PJDesk server has this account.")
        if (!CloudGate.allowCloud()) throw CloudAuthException("Waiting before asking pjdesk.com again.")
        val reply = try {
            post("$API/api/network/refresh", null, body)
        } catch (e: CloudAuthException) {
            noteTransport(e)
            throw e
        }
        CloudGate.noteOk()
        return session("", deviceId, reply)
    }

    /** Enrolls from a network key. A bound key also needs that account's email and password. */
    fun redeem(blob: String, passphrase: String, deviceId: String, deviceName: String, email: String = "", password: String = ""): Session {
        val opened = NetKey.open(blob, passphrase)
        val bearer = if (email.isNotBlank() && password.isNotBlank()) loginToken(email, password) else null
        val reply = post(
            "$API/api/network/redeem",
            bearer,
            JSONObject().put("secret", opened.secretHex).put("device_id", deviceId).put("device_name", deviceName),
        )
        val session = session("", deviceId, reply)
        return session.copy(
            address = session.address.ifBlank { opened.active },
            fingerprint = session.fingerprint.ifBlank { opened.activeFingerprint },
            standbyAddress = session.standbyAddress.ifBlank { opened.standby },
            standbyFingerprint = session.standbyFingerprint.ifBlank { opened.standbyFingerprint },
        )
    }

    private fun loginToken(email: String, password: String): String {
        val auth = authKeyHex(email, password)
        val login = post(
            "$API/api/login",
            null,
            JSONObject().put("email", email.trim().lowercase()).put("auth_key", auth).put("device", "PJDesk"),
        )
        return login.getString("token")
    }

    /** Clients do not call /api/ip. The server already knows its public address. */
    fun publicIp(): String = ""

    /** A session answered by the PJDesk server, not by pjdesk.com. */
    fun sessionFromServer(email: String, deviceId: String, reply: JSONObject): Session {
        val address = reply.optString("address")
        if (address.isBlank()) {
            throw CloudAuthException(NO_SERVER, "no_server", reply.optString("role").ifBlank { "owner" })
        }
        val computers = mutableListOf<SavedComputer>()
        val list = reply.optJSONArray("computers")
        if (list != null) {
            for (i in 0 until list.length()) {
                val o = list.optJSONObject(i) ?: continue
                computers.add(
                    SavedComputer(
                        id = o.optString("id"),
                        name = o.optString("name"),
                        osFamily = o.optString("os_family"),
                        osVersion = o.optString("os_version"),
                        hostname = o.optString("hostname"),
                        displayName = o.optString("display_name"),
                    ),
                )
            }
        }
        return Session(
            email = reply.optString("email").ifBlank { email.trim().lowercase() },
            access = reply.optString("access"),
            exp = reply.optLong("exp"),
            refresh = reply.optString("refresh"),
            deviceId = reply.optString("device_id").ifBlank { deviceId },
            address = address,
            fingerprint = reply.optString("fingerprint"),
            standbyAddress = reply.optString("standby"),
            standbyFingerprint = reply.optString("standbyFingerprint"),
            lan = reply.optString("lan").split(',').map { it.trim() }.filter { it.isNotEmpty() },
            publicIp = reply.optString("publicIp"),
            computers = computers,
            standbyLan = reply.optString("standbyLan").split(',').map { it.trim() }.filter { it.isNotEmpty() },
            standbyPublicIp = reply.optString("standbyPublicIp"),
        )
    }

    private fun noteTransport(e: CloudAuthException) {
        val message = e.message.orEmpty()
        val edge = message.contains("temporarily unavailable") || message.contains("temporarily rate limited") || message.contains("1027")
        if (e.status == 0 || edge || e.status >= 500) CloudGate.noteFail()
    }

    fun createJoinCode(refresh: String, deviceId: String): JoinCode {
        if (refresh.isBlank() || deviceId.isBlank()) throw CloudAuthException("Sign in again.")
        val reply = post(
            "$API/api/network/join-code",
            null,
            JSONObject().put("refresh", refresh).put("device_id", deviceId),
        )
        val code = reply.optString("code").filter { it.isDigit() }
        val id = reply.optString("id")
        if (!id.startsWith("jnc_") || code.length != 8) throw CloudAuthException("PJDesk did not return a code.")
        return JoinCode(id, code, reply.optLong("expires_at"))
    }

    fun listJoinCodes(refresh: String, deviceId: String): List<JoinCode> {
        if (refresh.isBlank() || deviceId.isBlank()) throw CloudAuthException("Sign in again.")
        val reply = get("$API/api/network/join-code", refresh, deviceId)
        val array = reply.optJSONArray("codes") ?: return emptyList()
        val out = ArrayList<JoinCode>(array.length())
        for (i in 0 until array.length()) {
            val item = array.optJSONObject(i) ?: continue
            out.add(JoinCode(item.optString("id"), "", item.optLong("expires_at")))
        }
        return out
    }

    fun revokeJoinCode(refresh: String, deviceId: String, id: String) {
        if (refresh.isBlank() || deviceId.isBlank()) throw CloudAuthException("Sign in again.")
        post(
            "$API/api/network/join-code/revoke",
            null,
            JSONObject().put("refresh", refresh).put("device_id", deviceId).put("id", id),
        )
    }

    fun offerSupport(code: String, pjdeskId: String) {
        val digits = code.filter { it.isDigit() }
        val id = pjdeskId.filter { it.isDigit() }
        if (digits.length != 8 || id.length != 9) throw CloudAuthException("This phone is not online yet, so a support code cannot be used.")
        post(
            "$API/api/support/offer",
            null,
            JSONObject().put("code_hash", sha256Hex(digits)).put("pjdesk_id", id),
        )
    }

    fun revokeSupport(code: String, pjdeskId: String) {
        val digits = code.filter { it.isDigit() }
        val id = pjdeskId.filter { it.isDigit() }
        if (digits.length != 8 || id.length != 9) return
        runCatching {
            post(
                "$API/api/support/revoke",
                null,
                JSONObject().put("code_hash", sha256Hex(digits)).put("pjdesk_id", id),
            )
        }
    }

    fun lookupSupport(code: String): String {
        val digits = code.filter { it.isDigit() }
        if (digits.length != 8) throw CloudAuthException("Enter the 8-digit code.")
        val reply = post("$API/api/support/lookup", null, JSONObject().put("code", digits))
        val id = reply.optString("id").filter { it.isDigit() }
        if (id.length != 9) throw CloudAuthException("That code is not valid.")
        return id
    }

    fun sha256Hex(text: String): String {
        val digest = MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8))
        return digest.joinToString("") { "%02x".format(it) }
    }

    fun enroll(code: String, deviceId: String, deviceName: String): Session {
        val reply = post(
            "$API/api/network/enroll",
            null,
            JSONObject().put("code", code).put("device_id", deviceId).put("device_name", deviceName),
        )
        return session("", deviceId, reply)
    }

    /** Computers uploaded with a refresh. Passwords are never included. */
    fun computersJson(computers: List<SavedComputer>): JSONArray {
        val array = JSONArray()
        computers.forEach { c ->
            array.put(
                JSONObject()
                    .put("id", c.id)
                    .put("name", c.nickname())
                    .put("hostname", c.hostname)
                    .put("os_family", c.osFamily)
                    .put("os_version", c.osVersion)
                    .put("display_name", c.displayName),
            )
        }
        return array
    }

    /**
     * Adds this phone to the account list, keeping a nickname already stored.
     * `uploadLocal` is false for a temporary sign-in, so a list already on this
     * phone is not sent to the account.
     */
    fun prepareEnroll(
        account: List<SavedComputer>,
        local: List<SavedComputer>,
        uploadLocal: Boolean,
        id: String,
        hostname: String,
        displayName: String,
        osVersion: String,
    ): List<SavedComputer> {
        val digits = id.filter { it.isDigit() }
        val base = (if (uploadLocal) local else account).toMutableList()
        if (uploadLocal && digits.length == 9) {
            val accountName = account.find { it.id == digits }?.nickname().orEmpty()
            val index = base.indexOfFirst { it.id == digits }
            if (index >= 0 && accountName.isNotBlank() && base[index].nickname().isBlank()) {
                base[index] = base[index].copy(name = accountName)
            }
        }
        return enrollComputer(base, digits, hostname, displayName, osVersion)
    }

    fun enrollComputer(
        computers: List<SavedComputer>,
        id: String,
        hostname: String,
        displayName: String,
        osVersion: String,
    ): List<SavedComputer> {
        if (id.length != 9) return computers
        val host = hostname.trim()
        val display = displayName.trim().let { if (it.equals(host, ignoreCase = true)) "" else it }
        val index = computers.indexOfFirst { it.id == id }
        if (index < 0) {
            return computers + SavedComputer(
                id = id,
                hostname = host,
                osFamily = "android",
                osVersion = osVersion,
                displayName = display,
            )
        }
        val cur = computers[index]
        return computers.toMutableList().also {
            it[index] = cur.copy(
                hostname = host.ifBlank { cur.hostname },
                osFamily = "android",
                osVersion = osVersion.ifBlank { cur.osVersion },
                displayName = display.ifBlank { cur.displayName },
            )
        }
    }

    /** Account named in an access token. The signature is not checked. This is not a sign-in. */
    fun emailFromAccessToken(token: String): String {
        val bodyHex = token.substringBefore('.', missingDelimiterValue = "")
        if (bodyHex.length < 2 || bodyHex.length % 2 != 0) return ""
        if (bodyHex.any { it !in '0'..'9' && it !in 'a'..'f' && it !in 'A'..'F' }) return ""
        val bytes = runCatching {
            ByteArray(bodyHex.length / 2) { i -> bodyHex.substring(i * 2, i * 2 + 2).toInt(16).toByte() }
        }.getOrNull() ?: return ""
        val text = runCatching { String(bytes, Charsets.UTF_8) }.getOrNull() ?: return ""
        val lines = text.split('\n')
        if (lines.size < 2 || lines[0] != "PJN1") return ""
        val email = lines[1].trim().lowercase()
        return if ('@' in email && ' ' !in email) email else ""
    }

    private fun session(email: String, deviceId: String, reply: JSONObject): Session {
        val network = reply.optJSONObject("network") ?: throw CloudAuthException("This account has no network yet.")
        val address = network.optString("active_address")
        if (address.isBlank()) throw CloudAuthException(NO_SERVER, "no_server", "owner")
        val computers = mutableListOf<SavedComputer>()
        val list = reply.optJSONArray("computers")
        if (list != null) {
            for (i in 0 until list.length()) {
                val o = list.getJSONObject(i)
                computers.add(
                    SavedComputer(
                        id = o.optString("id"),
                        name = o.optString("name"),
                        osFamily = o.optString("os_family"),
                        osVersion = o.optString("os_version"),
                        hostname = o.optString("hostname"),
                        displayName = o.optString("display_name"),
                    ),
                )
            }
        }
        val presence = mutableListOf<SavedComputer>()
        val seen = reply.optJSONArray("presence")
        if (seen != null) {
            for (i in 0 until seen.length()) {
                val o = seen.getJSONObject(i)
                presence.add(
                    SavedComputer(
                        id = o.optString("id").filter { it.isDigit() },
                        hostname = o.optString("hostname"),
                        osFamily = o.optString("os_family"),
                        osVersion = o.optString("os_version"),
                        consoleUser = o.optString("console_user"),
                        lastSeen = o.optLong("last_seen"),
                        online = presenceOnlineFlag(if (o.has("online") && !o.isNull("online")) o.get("online") else null),
                        displayName = o.optString("display_name"),
                        appVersion = o.optString("app_version"),
                    ),
                )
            }
        }
        return Session(
            email = email.trim().ifBlank { emailFromAccessToken(reply.optString("access")) },
            access = reply.optString("access"),
            exp = reply.optLong("exp"),
            refresh = reply.optString("refresh"),
            deviceId = reply.optString("device_id").ifBlank { deviceId },
            address = address,
            fingerprint = network.optString("active_fingerprint"),
            standbyAddress = network.optString("standby_address"),
            standbyFingerprint = network.optString("standby_fingerprint"),
            lan = network.optString("active_lan").split(',').map { it.trim() }.filter { it.isNotEmpty() },
            publicIp = network.optString("active_public_ip"),
            routes = network.optJSONObject("routes")?.toString().orEmpty(),
            standbyLan = network.optString("standby_lan").split(',').map { it.trim() }.filter { it.isNotEmpty() },
            standbyPublicIp = network.optString("standby_public_ip"),
            computers = mergePresence(computers, presence),
        )
    }

    /** Presence fills a saved row and adds computers that were never saved by hand. */
    private fun mergePresence(computers: List<SavedComputer>, presence: List<SavedComputer>): List<SavedComputer> {
        return applyPresence(
            computers,
            presence.map {
                PresenceReport(
                    id = it.id,
                    hostname = it.hostname,
                    osFamily = it.osFamily,
                    osVersion = it.osVersion,
                    consoleUser = it.consoleUser,
                    displayName = it.displayName,
                    lastSeen = it.lastSeen,
                    online = it.online,
                    appVersion = it.appVersion,
                )
            },
        )
    }

    private fun get(url: String, bearer: String, deviceId: String): JSONObject {
        val conn = (URL(url).openConnection() as HttpURLConnection).apply {
            requestMethod = "GET"
            connectTimeout = 25_000
            readTimeout = 25_000
            setRequestProperty("Authorization", "Bearer $bearer")
            if (deviceId.isNotBlank()) setRequestProperty("X-PJDesk-Device", deviceId)
        }
        val code = conn.responseCode
        val text = (if (code in 200..299) conn.inputStream else conn.errorStream)?.bufferedReader()?.readText().orEmpty()
        if (code !in 200..299) {
            val parsed = runCatching { JSONObject(text) }.getOrNull()
            val message = parsed?.optString("error").orEmpty()
            throw CloudAuthException(message.ifBlank { if (text.isBlank()) "PJDesk replied $code." else "PJDesk replied $code: ${text.take(160)}" })
        }
        return JSONObject(text)
    }

    private fun post(url: String, bearer: String?, body: JSONObject): JSONObject {
        val conn = (URL(url).openConnection() as HttpURLConnection).apply {
            requestMethod = "POST"
            connectTimeout = 25_000
            readTimeout = 25_000
            doOutput = true
            setRequestProperty("content-type", "application/json")
            if (bearer != null) setRequestProperty("Authorization", "Bearer $bearer")
        }
        conn.outputStream.use { it.write(body.toString().toByteArray()) }
        val code = conn.responseCode
        val text = (if (code in 200..299) conn.inputStream else conn.errorStream)?.bufferedReader()?.readText().orEmpty()
        if (code !in 200..299) {
            val type = conn.contentType.orEmpty()
            val html = type.contains("text/html", ignoreCase = true)
                || text.contains("<html", ignoreCase = true)
                || text.contains("error 1027", ignoreCase = true)
                || text.contains("temporarily rate limited", ignoreCase = true)
            if (html) {
                throw CloudAuthException(
                    "PJDesk Cloud is temporarily unavailable. This computer keeps working on the local network.",
                    status = code,
                )
            }
            val parsed = runCatching { JSONObject(text) }.getOrNull()
            val message = parsed?.optString("error").orEmpty()
            val retryAfter = conn.getHeaderField("Retry-After")?.trim()?.toLongOrNull()?.takeIf { it > 0 }
            throw CloudAuthException(
                message.ifBlank { "Could not sign in ($code)." },
                parsed?.optString("code").orEmpty(),
                parsed?.optString("role").orEmpty(),
                status = code,
                retryAfter = retryAfter,
            )
        }
        return JSONObject(text)
    }
}

class CloudAuthException(
    message: String,
    val code: String = "",
    val role: String = "",
    val loginToken: String = "",
    val status: Int = 0,
    val retryAfter: Long? = null,
) : Exception(message)

data class JoinCode(
    val id: String,
    val code: String,
    val expiresAt: Long,
)

data class Session(
    val email: String,
    val access: String,
    val exp: Long,
    val refresh: String,
    val deviceId: String,
    val address: String,
    val fingerprint: String,
    val standbyAddress: String,
    val standbyFingerprint: String,
    val lan: List<String> = emptyList(),
    val publicIp: String = "",
    val computers: List<SavedComputer>,
    val routes: String = "",
    val standbyLan: List<String> = emptyList(),
    val standbyPublicIp: String = "",
)
