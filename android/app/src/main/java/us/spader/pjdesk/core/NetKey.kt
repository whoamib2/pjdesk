package us.spader.pjdesk.core

import org.bouncycastle.crypto.generators.Argon2BytesGenerator
import org.bouncycastle.crypto.modes.ChaCha20Poly1305
import org.bouncycastle.crypto.params.Argon2Parameters
import org.bouncycastle.crypto.params.KeyParameter
import org.bouncycastle.crypto.params.ParametersWithIV
import java.util.Base64

/**
 * Opens a `pjdesk-net1:` blob. The layout matches pjdesk-proto. A wrong
 * passphrase and a tampered blob both fail closed. Nothing here is logged.
 */
object NetKey {
    private const val PREFIX = "pjdesk-net1:"

    data class Opened(
        val networkId: String,
        val secretHex: String,
        val active: String,
        val activeFingerprint: String,
        val standby: String,
        val standbyFingerprint: String,
    )

    /**
     * Pulls a `pjdesk-net1:` blob out of pasted text or a file name.
     * The result is still encrypted. A `host=` string is ignored.
     */
    fun blobIn(text: String): String? {
        val idx = text.indexOf(PREFIX)
        if (idx < 0) return null
        val rest = text.substring(idx + PREFIX.length)
        var end = rest.length
        for (i in rest.indices) {
            val c = rest[i]
            if (!(c.isLetterOrDigit() || c == '-' || c == '_')) {
                end = i
                break
            }
        }
        if (end < 16) return null
        return PREFIX + rest.substring(0, end)
    }

    fun kind(blob: String): String {
        val raw = decode(blob) ?: return ""
        return when (raw.getOrNull(1)?.toInt()) {
            1 -> "passphrase"
            2 -> "embedded"
            else -> ""
        }
    }

    fun open(blob: String, passphrase: String): Opened {
        val raw = decode(blob) ?: throw CloudAuthException("This network key is not valid.")
        if (raw.isEmpty() || raw[0].toInt() != 1) throw CloudAuthException("This network key is not valid.")
        val key: ByteArray
        val aadEnd: Int
        val nonceEnd: Int
        when (raw[1].toInt()) {
            1 -> {
                if (passphrase.isEmpty() || raw.size < 54 + 16) {
                    throw CloudAuthException("That network key could not be opened. Check the text and the passphrase.")
                }
                val salt = raw.copyOfRange(2, 18)
                val mem = le32(raw, 18)
                val iterations = le32(raw, 22)
                val lanes = le32(raw, 26)
                key = argon2id(passphrase, salt, mem, iterations, lanes)
                aadEnd = 30
                nonceEnd = 54
            }
            2 -> {
                if (raw.size < 58 + 16) throw CloudAuthException("This network key is not valid.")
                key = raw.copyOfRange(2, 34)
                aadEnd = 34
                nonceEnd = 58
            }
            else -> throw CloudAuthException("This network key is not valid.")
        }
        val nonce = raw.copyOfRange(aadEnd, nonceEnd)
        val ct = raw.copyOfRange(nonceEnd, raw.size)
        val plain = try {
            xchachaDecrypt(key, nonce, raw.copyOfRange(0, aadEnd), ct)
        } catch (_: Exception) {
            throw CloudAuthException("That network key could not be opened. Check the text and the passphrase.")
        } finally {
            key.fill(0)
        }
        return parse(plain)
    }

    private fun decode(blob: String): ByteArray? {
        val compact = blob.filter { !it.isWhitespace() }
        if (!compact.startsWith(PREFIX)) return null
        val body = compact.removePrefix(PREFIX)
        if (body.isEmpty() || body.length > 12_000) return null
        return try {
            Base64.getUrlDecoder().decode(body)
        } catch (_: IllegalArgumentException) {
            null
        }
    }

    private fun parse(plain: ByteArray): Opened {
        val lines = plain.toString(Charsets.UTF_8).split('\n')
        if (lines.size < 7 || lines[0] != "PJNK1" || lines[1].isEmpty() || lines[2].length != 64 || lines[3].isEmpty()) {
            throw CloudAuthException("This network key is not valid.")
        }
        return Opened(lines[1], lines[2], lines[3], lines[4], lines[5], lines[6])
    }

    private fun argon2id(passphrase: String, salt: ByteArray, memKib: Int, iterations: Int, lanes: Int): ByteArray {
        if (lanes !in 1..4 || iterations !in 1..8 || memKib < 8 * lanes || memKib > 64 * 1024) {
            throw CloudAuthException("This network key is not valid.")
        }
        val params = Argon2Parameters.Builder(Argon2Parameters.ARGON2_id)
            .withVersion(Argon2Parameters.ARGON2_VERSION_13)
            .withSalt(salt)
            .withMemoryAsKB(memKib)
            .withIterations(iterations)
            .withParallelism(lanes)
            .build()
        val gen = Argon2BytesGenerator()
        gen.init(params)
        val out = ByteArray(32)
        gen.generateBytes(passphrase.toByteArray(Charsets.UTF_8), out)
        return out
    }

    private fun xchachaDecrypt(key: ByteArray, nonce24: ByteArray, aad: ByteArray, ct: ByteArray): ByteArray {
        val sub = hchacha20(key, nonce24.copyOfRange(0, 16))
        val nonce12 = ByteArray(12)
        nonce24.copyInto(nonce12, 4, 16, 24)
        val cipher = ChaCha20Poly1305()
        cipher.init(false, ParametersWithIV(KeyParameter(sub), nonce12))
        cipher.processAADBytes(aad, 0, aad.size)
        val out = ByteArray(cipher.getOutputSize(ct.size))
        var n = cipher.processBytes(ct, 0, ct.size, out, 0)
        n += cipher.doFinal(out, n)
        sub.fill(0)
        return out.copyOf(n)
    }

    private fun hchacha20(key: ByteArray, nonce16: ByteArray): ByteArray {
        val s = IntArray(16)
        s[0] = 0x61707865
        s[1] = 0x3320646e
        s[2] = 0x79622d32
        s[3] = 0x6b206574
        for (i in 0 until 8) s[4 + i] = le32(key, i * 4)
        for (i in 0 until 4) s[12 + i] = le32(nonce16, i * 4)
        repeat(10) {
            quarter(s, 0, 4, 8, 12)
            quarter(s, 1, 5, 9, 13)
            quarter(s, 2, 6, 10, 14)
            quarter(s, 3, 7, 11, 15)
            quarter(s, 0, 5, 10, 15)
            quarter(s, 1, 6, 11, 12)
            quarter(s, 2, 7, 8, 13)
            quarter(s, 3, 4, 9, 14)
        }
        val out = ByteArray(32)
        for (i in 0 until 4) put32(out, i * 4, s[i])
        for (i in 0 until 4) put32(out, 16 + i * 4, s[12 + i])
        return out
    }

    private fun quarter(s: IntArray, a: Int, b: Int, c: Int, d: Int) {
        s[a] += s[b]; s[d] = s[d] xor s[a]; s[d] = s[d].rotateLeft(16)
        s[c] += s[d]; s[b] = s[b] xor s[c]; s[b] = s[b].rotateLeft(12)
        s[a] += s[b]; s[d] = s[d] xor s[a]; s[d] = s[d].rotateLeft(8)
        s[c] += s[d]; s[b] = s[b] xor s[c]; s[b] = s[b].rotateLeft(7)
    }

    private fun le32(b: ByteArray, i: Int): Int =
        (b[i].toInt() and 0xff) or
            ((b[i + 1].toInt() and 0xff) shl 8) or
            ((b[i + 2].toInt() and 0xff) shl 16) or
            ((b[i + 3].toInt() and 0xff) shl 24)

    private fun put32(b: ByteArray, i: Int, v: Int) {
        b[i] = v.toByte()
        b[i + 1] = (v ushr 8).toByte()
        b[i + 2] = (v ushr 16).toByte()
        b[i + 3] = (v ushr 24).toByte()
    }
}
