package us.spader.pjdesk

import android.app.Application
import kotlinx.coroutines.flow.MutableStateFlow
import us.spader.pjdesk.core.Engine
import us.spader.pjdesk.data.Store

class PjApp : Application() {
    lateinit var engine: Engine
        private set
    lateinit var store: Store
        private set

    /** Saved-password key. Present only while the app UI is unlocked. */
    @Volatile var vaultKey: ByteArray? = null
        private set

    val uiUnlocked = MutableStateFlow(false)

    /** The biometric system dialog is up, so a pause must not count as leaving the app. */
    @Volatile var bioPrompt = false

    @Volatile var backgroundAt = 0L

    override fun onCreate() {
        super.onCreate()
        AppLog.init(this)
        store = Store(this)
        engine = Engine(this)
        engine.start()
    }

    fun holdVaultKey(key: ByteArray) {
        vaultKey?.fill(0)
        vaultKey = key.copyOf()
    }

    fun clearVaultKey() {
        vaultKey?.fill(0)
        vaultKey = null
    }

    fun markUnlocked() {
        uiUnlocked.value = true
    }

    /** Locks the UI. The host service keeps its own token and is not stopped. */
    fun lockUi() {
        clearVaultKey()
        uiUnlocked.value = false
    }
}
