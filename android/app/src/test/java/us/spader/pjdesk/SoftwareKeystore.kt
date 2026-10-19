package us.spader.pjdesk

import android.security.keystore.KeyGenParameterSpec
import java.io.InputStream
import java.io.OutputStream
import java.security.Key
import java.security.KeyStoreSpi
import java.security.KeyStoreException
import java.security.Provider
import java.security.SecureRandom
import java.security.Security
import java.security.cert.Certificate
import java.util.Collections
import java.util.Date
import java.util.Enumeration
import javax.crypto.KeyGenerator
import javax.crypto.KeyGeneratorSpi
import javax.crypto.SecretKey

/**
 * Stand-in for the platform Android Keystore. Unit tests have no keystore
 * daemon; this provider keeps the AES key the way the real keystore does
 * across a new [us.spader.pjdesk.data.Store]. It is not in the app.
 */
object SoftwareKeystore {
    private val keys = mutableMapOf<String, SecretKey>()

    fun install() {
        keys.clear()
        if (Security.getProvider("AndroidKeyStore") == null) {
            Security.addProvider(SoftwareProvider())
        }
    }

    class SoftwareProvider : Provider("AndroidKeyStore", 1.0, "PJDesk test keystore") {
        init {
            put("KeyStore.AndroidKeyStore", SoftwareKeyStore::class.java.name)
            put("KeyGenerator.AES", SoftwareAesGenerator::class.java.name)
        }
    }

    class SoftwareKeyStore : KeyStoreSpi() {
        override fun engineGetKey(alias: String, password: CharArray?): Key? = keys[alias]
        override fun engineGetCertificateChain(alias: String?): Array<Certificate>? = null
        override fun engineGetCertificate(alias: String?): Certificate? = null
        override fun engineGetCreationDate(alias: String?): Date? = null
        override fun engineSetKeyEntry(alias: String, key: Key, password: CharArray?, chain: Array<out Certificate>?) {
            keys[alias] = key as SecretKey
        }
        override fun engineSetKeyEntry(alias: String?, key: ByteArray?, chain: Array<out Certificate>?) {
            throw KeyStoreException("unsupported")
        }
        override fun engineSetCertificateEntry(alias: String?, cert: Certificate?) = Unit
        override fun engineDeleteEntry(alias: String) {
            keys.remove(alias)
        }
        override fun engineAliases(): Enumeration<String> = Collections.enumeration(keys.keys)
        override fun engineContainsAlias(alias: String): Boolean = keys.containsKey(alias)
        override fun engineSize(): Int = keys.size
        override fun engineIsKeyEntry(alias: String): Boolean = keys.containsKey(alias)
        override fun engineIsCertificateEntry(alias: String?): Boolean = false
        override fun engineGetCertificateAlias(cert: Certificate?): String? = null
        override fun engineStore(stream: OutputStream?, password: CharArray?) = Unit
        override fun engineLoad(stream: InputStream?, password: CharArray?) = Unit
    }

    class SoftwareAesGenerator : KeyGeneratorSpi() {
        private var alias = "pjdesk-permanent"

        override fun engineInit(random: SecureRandom?) = Unit
        override fun engineInit(params: java.security.spec.AlgorithmParameterSpec?, random: SecureRandom?) {
            if (params is KeyGenParameterSpec) alias = params.keystoreAlias
        }
        override fun engineInit(keysize: Int, random: SecureRandom?) = Unit
        override fun engineGenerateKey(): SecretKey {
            val key = KeyGenerator.getInstance("AES", "SunJCE").apply { init(256) }.generateKey()
            keys[alias] = key
            return key
        }
    }
}
