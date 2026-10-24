package us.spader.pjdesk.core

import org.bouncycastle.crypto.modes.ChaCha20Poly1305
import org.bouncycastle.crypto.params.KeyParameter
import org.bouncycastle.crypto.params.ParametersWithIV
import org.json.JSONObject
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.PBEKeySpec
import javax.crypto.spec.SecretKeySpec

/** Rules for the app PIN and the auto-lock timer. The host keeps running while this is locked. */
object AppLockPolicy {
    const val MAX_FAILS = 5
    val DELAYS_SEC = intArrayOf(60, 120, 300)

    fun pinAcceptable(pin: String): Boolean = pin.length in 4..8 && pin.all { it.isDigit() }

    fun lockedOut(fails: Int): Boolean = fails >= MAX_FAILS

    fun relock(awayMs: Long, delayMs: Long): Boolean = delayMs > 0 && awayMs >= delayMs

    /** Wait after this many wrong PINs. The first four misses do not wait. */
    fun lockoutSeconds(fails: Int): Int = when {
        fails < MAX_FAILS -> 0
        fails == 5 -> 60
        fails == 6 -> 120
        fails == 7 -> 300
        fails == 8 -> 600
        else -> 1800
    }
}

/** Wraps the vault key with the app PIN. A wrong PIN fails the GCM check. */
object PinWrap {
    private const val ITERATIONS = 120_000

    fun newSalt(): ByteArray {
        val salt = ByteArray(16)
        SecureRandom().nextBytes(salt)
        return salt
    }

    fun wrap(vaultKey: ByteArray, pin: String, salt: ByteArray): String {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, SecretKeySpec(pinKey(pin, salt), "AES"))
        val iv = cipher.iv
        val body = cipher.doFinal(vaultKey)
        val out = ByteArray(1 + iv.size + body.size)
        out[0] = iv.size.toByte()
        iv.copyInto(out, 1)
        body.copyInto(out, 1 + iv.size)
        return Base64.getEncoder().encodeToString(out)
    }

    fun unwrap(stored: String, pin: String, salt: ByteArray): ByteArray? = runCatching {
        val raw = Base64.getDecoder().decode(stored)
        if (raw.size < 2) return null
        val ivLen = raw[0].toInt() and 0xff
        if (ivLen == 0 || raw.size <= 1 + ivLen) return null
        val iv = raw.copyOfRange(1, 1 + ivLen)
        val body = raw.copyOfRange(1 + ivLen, raw.size)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, SecretKeySpec(pinKey(pin, salt), "AES"), GCMParameterSpec(128, iv))
        cipher.doFinal(body)
    }.getOrNull()

    private fun pinKey(pin: String, salt: ByteArray): ByteArray {
        val spec = PBEKeySpec(pin.toCharArray(), salt, ITERATIONS, 256)
        return SecretKeyFactory.getInstance("PBKDF2WithHmacSHA256").generateSecret(spec).encoded
    }
}

/**
 * The account PIN. Only a salted slow hash is stored, sealed the same way as
 * the password vault so pjdesk.com cannot read it.
 */
object AccountPin {
    const val ITERATIONS = 200_000
    const val IDLE_DEFAULT = 300
    private val MAGIC = "PJLOCK01".toByteArray(Charsets.US_ASCII)

    data class Verifier(
        val salt: String,
        val hash: String,
        val iterations: Int,
        val idleSec: Int,
        val updated: Long,
    )

    fun hashPin(pin: String, salt: ByteArray, iterations: Int): ByteArray {
        val spec = PBEKeySpec(pin.toCharArray(), salt, iterations, 256)
        return SecretKeyFactory.getInstance("PBKDF2WithHmacSHA256").generateSecret(spec).encoded
    }

    fun matches(pin: String, saltB64: String, hashHex: String, iterations: Int): Boolean {
        if (!AppLockPolicy.pinAcceptable(pin) || iterations <= 0 || iterations > 600_000) return false
        val salt = runCatching { Base64.getDecoder().decode(saltB64) }.getOrNull() ?: return false
        val want = hexToBytes(hashHex) ?: return false
        val got = runCatching { hashPin(pin, salt, iterations) }.getOrNull() ?: return false
        return MessageDigest.isEqual(got, want)
    }

    fun create(pin: String, idleSec: Int, updated: Long): Verifier {
        val salt = ByteArray(16)
        SecureRandom().nextBytes(salt)
        val hash = hashPin(pin, salt, ITERATIONS)
        val idle = when {
            idleSec <= 0 -> IDLE_DEFAULT
            idleSec < 60 -> 60
            idleSec > 24 * 60 * 60 -> 24 * 60 * 60
            else -> idleSec
        }
        return Verifier(
            Base64.getEncoder().encodeToString(salt),
            hash.joinToString("") { "%02x".format(it) },
            ITERATIONS,
            idle,
            updated.coerceAtLeast(1),
        )
    }

    fun seal(key: ByteArray, verifier: Verifier): String {
        val nonce = ByteArray(12)
        SecureRandom().nextBytes(nonce)
        val plain = JSONObject()
            .put("salt", verifier.salt)
            .put("hash", verifier.hash)
            .put("iterations", verifier.iterations)
            .put("idle_sec", verifier.idleSec)
            .put("updated", verifier.updated)
            .toString()
            .toByteArray(Charsets.UTF_8)
        val ct = crypt(true, key, nonce, plain)
        val packed = ByteArray(MAGIC.size + nonce.size + ct.size)
        MAGIC.copyInto(packed)
        nonce.copyInto(packed, MAGIC.size)
        ct.copyInto(packed, MAGIC.size + nonce.size)
        return Base64.getEncoder().encodeToString(packed)
    }

    fun open(key: ByteArray, text: String): Verifier {
        val raw = Base64.getDecoder().decode(text.trim())
        if (raw.size < MAGIC.size + 12 + 16 || !raw.copyOfRange(0, MAGIC.size).contentEquals(MAGIC)) {
            throw IllegalArgumentException("that app lock is not valid")
        }
        val nonce = raw.copyOfRange(MAGIC.size, MAGIC.size + 12)
        val ct = raw.copyOfRange(MAGIC.size + 12, raw.size)
        val obj = JSONObject(crypt(false, key, nonce, ct).toString(Charsets.UTF_8))
        return Verifier(
            obj.getString("salt"),
            obj.getString("hash"),
            obj.getInt("iterations"),
            obj.optInt("idle_sec", IDLE_DEFAULT),
            obj.optLong("updated"),
        )
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

    private fun hexToBytes(hex: String): ByteArray? {
        val text = hex.trim()
        if (text.length != 64) return null
        return runCatching {
            ByteArray(32) { i -> text.substring(i * 2, i * 2 + 2).toInt(16).toByte() }
        }.getOrNull()
    }
}
