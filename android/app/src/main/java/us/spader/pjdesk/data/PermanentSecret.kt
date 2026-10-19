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
 * The permanent password, encrypted with an AES-GCM key that lives in the
 * Android Keystore. SPAKE2 needs the secret itself on the host, so this is
 * not a password hash. Only the ciphertext is written to preferences.
 */
internal class PermanentSecret(context: Context) {
    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    fun read(): String {
        val stored = prefs.getString(FIELD, null) ?: return ""
        return decrypt(stored).orEmpty()
    }

    /** Empty [password] removes the secret. Returns false when the write did not land. */
    fun write(password: String): Boolean {
        val editor = prefs.edit()
        if (password.isEmpty()) {
            editor.remove(FIELD)
        } else {
            val sealed = encrypt(password) ?: return false
            editor.putString(FIELD, sealed)
        }
        return editor.commit()
    }

    private fun encrypt(password: String): String? = runCatching {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val iv = cipher.iv
        val body = cipher.doFinal(password.toByteArray(Charsets.UTF_8))
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
        (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
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
        const val ALIAS = "pjdesk-permanent"
        const val PREFS = "pjdesk-secret"
        const val FIELD = "permanent"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
    }
}
