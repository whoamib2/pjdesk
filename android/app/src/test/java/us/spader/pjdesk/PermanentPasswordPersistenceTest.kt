package us.spader.pjdesk

import android.content.Context
import android.os.Looper
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import us.spader.pjdesk.core.Engine
import us.spader.pjdesk.data.Store
import java.io.File

/**
 * Saves the permanent password, then builds a new [Engine] the way a process
 * restart does. The string that engine gives the host is the permanent slot
 * SPAKE2 accepts in `permanent_password_survives_restart_and_authenticates`.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [33])
class PermanentPasswordPersistenceTest {
    private val context: Context
        get() = ApplicationProvider.getApplicationContext()

    @Before
    fun keystore() {
        SoftwareKeystore.install()
    }

    @Test
    fun saveSurvivesANewEngineAndIsWhatTheHostIsGiven() {
        val saved = "correct horse"
        val engine = Engine(context)
        assertTrue(engine.setPermanent(saved))
        assertTrue(engine.state.value.hasPermanent)
        assertEquals("Permanent password saved.", engine.state.value.notice)
        assertEquals("prompt", engine.state.value.approval)

        // A registration that has not caught up must not clear the saved password.
        engine.onEvent("""{"event":"registered","permanent":false,"password":"onetimeaa","approval":"prompt"}""")
        shadowOf(Looper.getMainLooper()).idle()
        assertTrue(engine.state.value.hasPermanent)
        assertEquals(saved, engine.permanentForHost())

        engine.setApproval("unattended")
        assertEquals("unattended", engine.state.value.approval)

        val restarted = Engine(context)
        assertTrue(restarted.state.value.hasPermanent)
        assertEquals("unattended", restarted.state.value.approval)
        assertEquals(saved, restarted.permanentForHost())
        assertFalse(restarted.permanentForHost() == "wrong horse battery")

        val prefs = File(context.applicationInfo.dataDir, "shared_prefs")
        val dumped = prefs.walkTopDown().filter { it.isFile }.joinToString("\n") { it.readText() }
        assertFalse(dumped.contains(saved))
    }

    @Test
    fun thisPhonesPasswordIsAVaultEntry() {
        val store = Store(context)
        assertEquals(0L, store.permanentPublished)
        assertTrue(store.stampPassword("482636105", "correct horse"))
        store.permanentPublished = 50
        val entry = store.vaultEntries().single { it.id == "482636105" }
        assertEquals("correct horse", entry.password)
        assertTrue(entry.updated > 0)
        val again = Store(context)
        assertEquals(50L, again.permanentPublished)
        assertEquals("correct horse", again.computerPassword("482636105"))
    }

    @Test
    fun clearingSurvivesANewEngine() {
        val engine = Engine(context)
        assertTrue(engine.setPermanent("correct horse"))
        engine.setApproval("unattended")
        assertTrue(engine.setPermanent(""))
        assertEquals("Permanent password removed.", engine.state.value.notice)
        assertFalse(engine.state.value.hasPermanent)
        assertEquals("prompt", engine.state.value.approval)

        val restarted = Engine(context)
        assertFalse(restarted.state.value.hasPermanent)
        assertEquals("", restarted.permanentForHost())
        assertEquals("prompt", restarted.state.value.approval)
    }

    @Test
    fun plainPreferenceIsMovedIntoTheKeystore() {
        val saved = "correct horse"
        val plain = context.getSharedPreferences("pjdesk", Context.MODE_PRIVATE)
        assertTrue(plain.edit().putString("permanent", saved).commit())

        val store = Store(context)
        assertEquals(saved, store.permanent)
        assertFalse(plain.contains("permanent"))

        val prefs = File(context.applicationInfo.dataDir, "shared_prefs")
        val dumped = prefs.walkTopDown().filter { it.isFile }.joinToString("\n") { it.readText() }
        assertFalse(dumped.contains(saved))
    }
}
