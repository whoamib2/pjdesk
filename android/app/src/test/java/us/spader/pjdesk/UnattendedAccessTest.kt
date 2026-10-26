package us.spader.pjdesk

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import us.spader.pjdesk.core.Engine
import us.spader.pjdesk.data.Store

/**
 * The saved unattended-access switch survives a process start. Accessibility
 * is not enabled in this test, which is the case that used to write it back off.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [33])
class UnattendedAccessTest {
    private val context: Context
        get() = ApplicationProvider.getApplicationContext()

    @Before
    fun keystore() {
        SoftwareKeystore.install()
        context.getSharedPreferences("pjdesk", Context.MODE_PRIVATE).edit().clear().commit()
    }

    @Test
    fun anOldUnattendedChoiceTurnsTheSwitchOnOnce() {
        context.getSharedPreferences("pjdesk", Context.MODE_PRIVATE)
            .edit()
            .putString("approval", "unattended")
            .commit()
        val engine = Engine(context)
        assertTrue(engine.state.value.unattendedAccess)
        assertTrue(Store(context).unattendedAccess)
        assertEquals("unattended", engine.state.value.approval)

        Store(context).unattendedAccess = false
        val again = Engine(context)
        assertFalse(again.state.value.unattendedAccess)
        assertEquals("unattended", again.state.value.approval)
    }

    @Test
    fun startupSyncDoesNotClearASavedSwitch() {
        Store(context).unattendedAccess = true
        val engine = Engine(context)
        engine.syncUnattendedAccess()
        assertTrue(Store(context).unattendedAccess)
        assertTrue(engine.state.value.unattendedAccess)
    }
}
