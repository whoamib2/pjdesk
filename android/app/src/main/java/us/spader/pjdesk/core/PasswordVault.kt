package us.spader.pjdesk.core

import org.bouncycastle.crypto.modes.ChaCha20Poly1305
import org.bouncycastle.crypto.params.KeyParameter
import org.bouncycastle.crypto.params.ParametersWithIV
import org.json.JSONObject
import java.security.SecureRandom
import java.util.Base64

/** One saved computer password inside the sealed account vault. An empty password is a deletion. */
data class VaultEntry(val id: String, val password: String, val updated: Long)

data class VaultMerge(val entries: List<VaultEntry>, val localChanged: Boolean, val remoteChanged: Boolean)

/**
 * PJVAULT1 blobs. The layout matches the Windows client: magic, 12-byte nonce,
 * ChaCha20-Poly1305 ciphertext. The account service never sees the key.
 */
object PasswordVault {
    private val MAGIC = "PJVAULT1".toByteArray(Charsets.US_ASCII)

    fun seal(key: ByteArray, entries: List<VaultEntry>): String {
        val nonce = ByteArray(12)
        SecureRandom().nextBytes(nonce)
        return sealWithNonce(key, entries, nonce)
    }

    fun sealWithNonce(key: ByteArray, entries: List<VaultEntry>, nonce: ByteArray): String {
        val plain = encode(entries).toByteArray(Charsets.UTF_8)
        val ct = crypt(true, key, nonce, plain)
        val packed = ByteArray(MAGIC.size + nonce.size + ct.size)
        MAGIC.copyInto(packed)
        nonce.copyInto(packed, MAGIC.size)
        ct.copyInto(packed, MAGIC.size + nonce.size)
        return Base64.getEncoder().encodeToString(packed)
    }

    fun open(key: ByteArray, text: String): List<VaultEntry> {
        val raw = Base64.getDecoder().decode(text.trim())
        if (raw.size < MAGIC.size + 12 + 16 || !raw.copyOfRange(0, MAGIC.size).contentEquals(MAGIC)) {
            throw IllegalArgumentException("that password copy is not valid")
        }
        val nonce = raw.copyOfRange(MAGIC.size, MAGIC.size + 12)
        val ct = raw.copyOfRange(MAGIC.size + 12, raw.size)
        val plain = crypt(false, key, nonce, ct)
        return decode(plain.toString(Charsets.UTF_8))
    }

    fun merge(local: List<VaultEntry>, remote: List<VaultEntry>, now: Long): VaultMerge {
        val ids = (local.map { it.id } + remote.map { it.id }).filter { it.length == 9 }.toSortedSet()
        val entries = mutableListOf<VaultEntry>()
        var localChanged = false
        var remoteChanged = false
        for (id in ids) {
            val left = local.find { it.id == id }
            val right = remote.find { it.id == id }
            val chosen = when {
                left == null && right != null -> {
                    localChanged = true
                    right
                }
                left != null && right == null -> {
                    if (left.password.isEmpty() && left.updated == 0L) continue
                    var kept = left
                    if (kept.updated == 0L) {
                        kept = kept.copy(updated = now.coerceAtLeast(1))
                        localChanged = true
                    }
                    remoteChanged = true
                    kept
                }
                left != null && right != null -> when {
                    right.updated > left.updated -> right
                    left.updated > right.updated -> left
                    right.password.isEmpty() && left.password.isNotEmpty() -> left
                    left.password.isEmpty() && right.password.isNotEmpty() -> right
                    else -> left
                }
                else -> continue
            }
            if (left != chosen) localChanged = true
            if (right != chosen) remoteChanged = true
            entries.add(chosen)
        }
        return VaultMerge(entries, localChanged, remoteChanged)
    }

    private fun encode(entries: List<VaultEntry>): String {
        val body = entries.filter { it.id.length == 9 }.sortedBy { it.id }.joinToString(",") { entry ->
            """{"id":"${escape(entry.id)}","password":"${escape(entry.password)}","updated":${entry.updated}}"""
        }
        return """{"entries":[$body]}"""
    }

    private fun decode(text: String): List<VaultEntry> {
        val array = JSONObject(text).optJSONArray("entries") ?: return emptyList()
        return buildList {
            for (i in 0 until array.length()) {
                val row = array.getJSONObject(i)
                val id = row.optString("id").filter { it.isDigit() }
                if (id.length == 9) add(VaultEntry(id, row.optString("password"), row.optLong("updated")))
            }
        }
    }

    private fun escape(text: String): String = buildString {
        for (c in text) {
            when (c) {
                '\\' -> append("\\\\")
                '"' -> append("\\\"")
                '\n' -> append("\\n")
                '\r' -> append("\\r")
                else -> if (c.code < 0x20) append("\\u%04x".format(c.code)) else append(c)
            }
        }
    }

    private fun crypt(encrypt: Boolean, key: ByteArray, nonce: ByteArray, input: ByteArray): ByteArray {
        val cipher = ChaCha20Poly1305()
        cipher.init(encrypt, ParametersWithIV(KeyParameter(key), nonce))
        cipher.processAADBytes(MAGIC, 0, MAGIC.size)
        val out = ByteArray(cipher.getOutputSize(input.size))
        var n = cipher.processBytes(input, 0, input.size, out, 0)
        n += cipher.doFinal(out, n)
        return out.copyOf(n)
    }
}
