package us.spader.pjdesk.core

import android.content.Context
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import java.security.KeyStore
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Unlocks the saved-password key with a fingerprint or face.
 * A class-3 biometric is required by the Keystore key itself. A phone that only
 * has a class-2 face sensor still gets a biometric prompt; Android does not let
 * that sensor authorize a Keystore key, so that copy uses a hardware-backed key
 * without a per-use biometric check. The app PIN is the fallback either way.
 */
object BioVault {
    private const val ALIAS = "pjdesk-vault-bio"
    private const val WEAK_ALIAS = "pjdesk-vault-bio-weak"
    private const val STORE = "AndroidKeyStore"
    private const val STRONG = BiometricManager.Authenticators.BIOMETRIC_STRONG
    private const val ANY = BiometricManager.Authenticators.BIOMETRIC_STRONG or
        BiometricManager.Authenticators.BIOMETRIC_WEAK

    fun available(context: Context): Boolean =
        BiometricManager.from(context).canAuthenticate(ANY) == BiometricManager.BIOMETRIC_SUCCESS

    fun forget() {
        runCatching {
            val ks = KeyStore.getInstance(STORE)
            ks.load(null)
            if (ks.containsAlias(ALIAS)) ks.deleteEntry(ALIAS)
            if (ks.containsAlias(WEAK_ALIAS)) ks.deleteEntry(WEAK_ALIAS)
        }
    }

    fun sealWithAuth(
        activity: FragmentActivity,
        vaultKey: ByteArray,
        onSealed: (String) -> Unit,
        onCancel: () -> Unit,
        onError: (String) -> Unit,
    ) {
        if (strong(activity)) {
            val cipher = try {
                ensureStrongKey()
                cipher().also { it.init(Cipher.ENCRYPT_MODE, secret(ALIAS)) }
            } catch (e: Exception) {
                onError(e.message ?: "Biometrics are not available on this phone.")
                return
            }
            prompt(
                activity,
                "Enable biometrics",
                "Confirm your fingerprint or face so PJDesk can unlock saved passwords.",
                STRONG,
                cipher,
                onSuccess = { authed ->
                    if (authed == null) {
                        onError("Biometrics did not unlock the key.")
                        return@prompt
                    }
                    runCatching { onSealed("strong:" + pack(authed, authed.doFinal(vaultKey))) }
                        .onFailure { onError(it.message ?: "Could not enable biometrics.") }
                },
                onCancel = onCancel,
                onError = onError,
            )
            return
        }
        prompt(
            activity,
            "Enable biometrics",
            "Confirm your fingerprint or face so PJDesk can unlock saved passwords.",
            ANY,
            null,
            onSuccess = {
                runCatching { onSealed("weak:" + weakSeal(vaultKey)) }
                    .onFailure { onError(it.message ?: "Could not enable biometrics.") }
            },
            onCancel = onCancel,
            onError = onError,
        )
    }

    fun openWithAuth(
        activity: FragmentActivity,
        stored: String,
        onKey: (ByteArray) -> Unit,
        onCancel: () -> Unit,
        onError: (String) -> Unit,
    ) {
        if (stored.startsWith("weak:")) {
            val payload = stored.removePrefix("weak:")
            prompt(
                activity,
                "Unlock PJDesk",
                "Use your fingerprint or face. The PIN still works.",
                ANY,
                null,
                onSuccess = {
                    runCatching { onKey(weakOpen(payload)) }
                        .onFailure { onError("Biometrics could not unlock saved passwords.") }
                },
                onCancel = onCancel,
                onError = onError,
            )
            return
        }
        val bodyText = stored.removePrefix("strong:")
        val opened = runCatching { decryptCipher(bodyText) }
        val (cipher, body) = opened.getOrElse {
            onError("Biometrics need to be turned on again in Settings.")
            return
        }
        prompt(
            activity,
            "Unlock PJDesk",
            "Use your fingerprint or face. The PIN still works.",
            STRONG,
            cipher,
            onSuccess = { authed ->
                if (authed == null) {
                    onError("Biometrics did not unlock the key.")
                    return@prompt
                }
                runCatching { onKey(authed.doFinal(body)) }
                    .onFailure { onError("Biometrics could not unlock saved passwords.") }
            },
            onCancel = onCancel,
            onError = onError,
        )
    }

    private fun strong(context: Context): Boolean =
        BiometricManager.from(context).canAuthenticate(STRONG) == BiometricManager.BIOMETRIC_SUCCESS

    /**
     * Confirms the person at the phone. This does not unlock the password vault.
     * [negative] is the button that leaves the prompt, usually "Use PIN".
     */
    fun confirm(
        activity: FragmentActivity,
        title: String,
        subtitle: String,
        negative: String,
        onSuccess: () -> Unit,
        onCancel: () -> Unit,
        onError: (String) -> Unit,
    ) {
        prompt(
            activity,
            title,
            subtitle,
            ANY,
            null,
            onSuccess = { onSuccess() },
            onCancel = onCancel,
            onError = onError,
            negative = negative,
        )
    }

    private fun prompt(
        activity: FragmentActivity,
        title: String,
        subtitle: String,
        authenticators: Int,
        cipher: Cipher?,
        onSuccess: (Cipher?) -> Unit,
        onCancel: () -> Unit,
        onError: (String) -> Unit,
        negative: String = "Use PIN",
    ) {
        val prompt = BiometricPrompt(
            activity,
            ContextCompat.getMainExecutor(activity),
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    onSuccess(result.cryptoObject?.cipher ?: cipher)
                }

                override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                    if (errorCode == BiometricPrompt.ERROR_NEGATIVE_BUTTON ||
                        errorCode == BiometricPrompt.ERROR_USER_CANCELED ||
                        errorCode == BiometricPrompt.ERROR_CANCELED
                    ) {
                        onCancel()
                    } else {
                        onError(errString.toString().ifBlank { "Biometrics were not accepted." })
                    }
                }
            },
        )
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle(title)
            .setSubtitle(subtitle)
            .setNegativeButtonText(negative)
            .setAllowedAuthenticators(authenticators)
            .build()
        if (cipher == null) prompt.authenticate(info) else prompt.authenticate(info, BiometricPrompt.CryptoObject(cipher))
    }

    private fun ensureStrongKey() {
        val ks = KeyStore.getInstance(STORE)
        ks.load(null)
        if (ks.containsAlias(ALIAS)) return
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, STORE)
        val builder = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .setUserAuthenticationRequired(true)
        if (Build.VERSION.SDK_INT >= 30) {
            builder.setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
            builder.setInvalidatedByBiometricEnrollment(false)
        } else {
            @Suppress("DEPRECATION")
            builder.setUserAuthenticationValidityDurationSeconds(-1)
        }
        gen.init(builder.build())
        gen.generateKey()
    }

    private fun secret(alias: String): SecretKey {
        val ks = KeyStore.getInstance(STORE)
        ks.load(null)
        (ks.getKey(alias, null) as? SecretKey)?.let { return it }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, STORE)
        gen.init(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return gen.generateKey()
    }

    private fun cipher(): Cipher = Cipher.getInstance("AES/GCM/NoPadding")

    private fun decryptCipher(stored: String): Pair<Cipher, ByteArray> {
        val raw = Base64.getDecoder().decode(stored)
        if (raw.size < 2) error("stored biometric wrap is short")
        val ivLen = raw[0].toInt() and 0xff
        if (ivLen == 0 || raw.size <= 1 + ivLen) error("stored biometric wrap is short")
        val iv = raw.copyOfRange(1, 1 + ivLen)
        val body = raw.copyOfRange(1 + ivLen, raw.size)
        val cipher = cipher()
        cipher.init(Cipher.DECRYPT_MODE, secret(ALIAS), GCMParameterSpec(128, iv))
        return cipher to body
    }

    private fun weakSeal(vaultKey: ByteArray): String {
        val cipher = cipher()
        cipher.init(Cipher.ENCRYPT_MODE, secret(WEAK_ALIAS))
        return pack(cipher, cipher.doFinal(vaultKey))
    }

    private fun weakOpen(stored: String): ByteArray {
        val raw = Base64.getDecoder().decode(stored)
        if (raw.size < 2) error("stored biometric wrap is short")
        val ivLen = raw[0].toInt() and 0xff
        if (ivLen == 0 || raw.size <= 1 + ivLen) error("stored biometric wrap is short")
        val iv = raw.copyOfRange(1, 1 + ivLen)
        val body = raw.copyOfRange(1 + ivLen, raw.size)
        val cipher = cipher()
        cipher.init(Cipher.DECRYPT_MODE, secret(WEAK_ALIAS), GCMParameterSpec(128, iv))
        return cipher.doFinal(body)
    }

    private fun pack(cipher: Cipher, body: ByteArray): String {
        val iv = cipher.iv
        val out = ByteArray(1 + iv.size + body.size)
        out[0] = iv.size.toByte()
        iv.copyInto(out, 1)
        body.copyInto(out, 1 + iv.size)
        return Base64.getEncoder().encodeToString(out)
    }
}
