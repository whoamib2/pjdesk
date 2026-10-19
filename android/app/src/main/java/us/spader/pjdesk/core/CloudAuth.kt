package us.spader.pjdesk.core

import org.json.JSONArray
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest
import javax.crypto.Mac
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.PBEKeySpec
import javax.crypto.spec.SecretKeySpec

/**
 * PJDesk account sign-in. The 600,000-round derivation must run off the UI
 * thread. Only the auth key is sent. The sync key is derived and discarded
 * here, because this phone does not seal a server backup.
 */
object CloudAuth {
    const val API = "https://pjdesk.com"

    fun canonical(email: String, networkId: String, fingerprints: List<String>, deviceId: String, exp: Long): String {
        val fps = fingerprints.map { normalizeFingerprint(it) }.filter { it.isNotEmpty() }.toSortedSet().joinToString(",")
        return "PJN1\n${email.trim().lowercase()}\n${networkId.trim()}\n$fps\n${deviceId.trim()}\n$exp\n"
    }

    fun normalizeFingerprint(text: String): String {
        val raw = text.trim().removePrefix("sha256:").removePrefix("SHA256:")
        return raw.lowercase().filter { it.isDigit() || it in 'a'..'f' }
    }

    fun authKeyHex(email: String, password: String): String {
        val salt = MessageDigest.getInstance("SHA-256")
            .digest("pjdesk-cloud-v1\u0000${email.trim().lowercase()}".toByteArray())
        val spec = PBEKeySpec(password.toCharArray(), salt, 600_000, 256)
        val master = SecretKeyFactory.getInstance("PBKDF2WithHmacSHA256").generateSecret(spec).encoded
        return hmac(master, "pjdesk-cloud-auth").joinToString("") { "%02x".format(it) }
    }

    private fun hmac(key: ByteArray, data: String): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        return mac.doFinal(data.toByteArray())
    }

    fun signIn(email: String, password: String, deviceId: String, deviceName: String): Session {
        val auth = authKeyHex(email, password)
        val login = post(
            "$API/api/login",
            null,
            JSONObject().put("email", email.trim().lowercase()).put("auth_key", auth).put("device", "PJDesk on $deviceName"),
        )
        val token = login.getString("token")
        val reply = post(
            "$API/api/network/token",
            token,
            JSONObject().put("device_id", deviceId).put("device_name", deviceName),
        )
        return session(email.trim().lowercase(), deviceId, reply)
    }

    fun refresh(refresh: String, deviceId: String): Session {
        val reply = post(
            "$API/api/network/refresh",
            null,
            JSONObject().put("refresh", refresh).put("device_id", deviceId),
        )
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
                    .put("name", c.name)
                    .put("os_family", c.osFamily)
                    .put("os_version", c.osVersion),
            )
        }
        return array
    }

    private fun session(email: String, deviceId: String, reply: JSONObject): Session {
        val network = reply.optJSONObject("network") ?: throw CloudAuthException("This account has no network yet.")
        val address = network.optString("active_address")
        if (address.isBlank()) throw CloudAuthException("Set the server address on pjdesk.com, then sign in again.")
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
                    ),
                )
            }
        }
        return Session(
            email = email,
            access = reply.optString("access"),
            exp = reply.optLong("exp"),
            refresh = reply.optString("refresh"),
            deviceId = reply.optString("device_id").ifBlank { deviceId },
            address = address,
            fingerprint = network.optString("active_fingerprint"),
            standbyAddress = network.optString("standby_address"),
            standbyFingerprint = network.optString("standby_fingerprint"),
            computers = computers,
        )
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
            val message = runCatching { JSONObject(text).optString("error") }.getOrDefault("")
            throw CloudAuthException(message.ifBlank { "Could not sign in ($code)." })
        }
        return JSONObject(text)
    }
}

class CloudAuthException(message: String) : Exception(message)

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
    val computers: List<SavedComputer>,
)
