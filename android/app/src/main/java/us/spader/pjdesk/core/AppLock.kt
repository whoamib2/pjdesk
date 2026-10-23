package us.spader.pjdesk.core

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
