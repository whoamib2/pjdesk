package us.spader.pjdesk.data

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * One short secret sealed with an Android Keystore AES-GCM key. The refresh
 * token and per-computer passwords use this. The permanent host password keeps
 * its own helper so this change cannot move that ciphertext.
 */
internal class KeystoreBlob(
    context: Context,
    private val alias: String,
    private val field: String,
) {
    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun read(): String {
        val stored = prefs.getString(field, null) ?: return ""
        return decrypt(stored).orEmpty()
    }

    fun write(value: String): Boolean {
        val editor = prefs.edit()
        if (value.isEmpty()) {
            editor.remove(field)
        } else {
            val sealed = encrypt(value) ?: return false
            editor.putString(field, sealed)
        }
        return editor.commit()
    }

    private fun encrypt(value: String): String? = runCatching {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val iv = cipher.iv
        val body = cipher.doFinal(value.toByteArray(Charsets.UTF_8))
        val out = ByteArray(1 + iv.size + body.size)
        out[0] = iv.size.toByte()
        System.arraycopy(iv, 0, out, 1, iv.size)
        System.arraycopy(body, 0, out, 1 + iv.size, body.size)
        Base64.encodeToString(out, Base64.NO_WRAP)
    }.getOrNull()

    private fun decrypt(stored: String): String? = runCatching {
        val raw = Base64.decode(stored, Base64.NO_WRAP)
        if (raw.size < 2) return null
        val ivLen = raw[0].toInt() and 0xff
        if (ivLen == 0 || raw.size <= 1 + ivLen) return null
        val iv = raw.copyOfRange(1, 1 + ivLen)
        val body = raw.copyOfRange(1 + ivLen, raw.size)
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, iv))
        cipher.doFinal(body).toString(Charsets.UTF_8)
    }.getOrNull()

    private fun key(): SecretKey {
        val store = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .setRandomizedEncryptionRequired(true)
                .build(),
        )
        return generator.generateKey()
    }

    private companion object {
        const val ANDROID_KEYSTORE = "AndroidKeyStore"
        const val PREFS = "pjdesk-secret"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
    }
}
