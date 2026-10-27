package us.spader.pjdesk.core

import us.spader.pjdesk.AppLog
import us.spader.pjdesk.data.Store

/** Pulls and, when asked, uploads the sealed saved-password vault. The key never leaves the phone. */
object PasswordSync {
    fun pull(store: Store, key: ByteArray, refresh: String, deviceId: String, upload: Boolean): Boolean {
        if (!CloudGate.serverUp) return false
        val remoteText = try {
            CloudAuth.fetchPasswordVault(refresh, deviceId)
        } catch (e: Exception) {
            AppLog.e("password vault download failed: ${e.message}")
            return false
        }
        val remote = if (remoteText.isBlank()) {
            emptyList()
        } else {
            try {
                PasswordVault.open(key, remoteText)
            } catch (e: Exception) {
                AppLog.e("password vault could not be opened")
                throw VaultKeyRejected()
            }
        }
        val now = System.currentTimeMillis() / 1000
        val merged = PasswordVault.merge(store.vaultEntries(), remote, now)
        if (merged.localChanged) store.applyVault(merged.entries)
        if (upload && merged.remoteChanged) {
            try {
                CloudAuth.putPasswordVault(refresh, deviceId, PasswordVault.seal(key, merged.entries))
                AppLog.i("password vault synced (${merged.entries.size} computers)")
            } catch (e: Exception) {
                AppLog.e("password vault upload failed: ${e.message}")
            }
        } else if (merged.localChanged) {
            AppLog.i("password vault synced (${merged.entries.size} computers)")
        }
        return merged.localChanged
    }
}

class VaultKeyRejected : Exception("Saved passwords need your account password again.")
