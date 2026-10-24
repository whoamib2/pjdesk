package us.spader.pjdesk

import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import us.spader.pjdesk.core.RemoteOptions
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import us.spader.pjdesk.data.Store

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [33])
class RemoteOptionsStoreTest {
    @Test
    fun defaultsStayGlobalAndOneComputerCanOverride() {
        val store = Store(ApplicationProvider.getApplicationContext())
        store.pointerSpeed = 1.5f
        val global = store.remoteGlobal()
        assertEquals(1.5f, global.pointerSpeed, 0.01f)
        assertEquals(false, global.dragLock)
        store.saveRemoteOverride("902488771", global.copy(dragLock = true, quality = "speed"))
        val over = store.remoteOverride("902488771")
        assertTrue(over != null && over.dragLock && over.quality == "speed")
        assertEquals(false, store.remoteGlobal().dragLock)
        store.saveRemoteGlobal(global.copy(invertScroll = true))
        assertTrue(store.remoteGlobal().invertScroll)
        assertEquals(false, store.remoteOverride("902488771")!!.invertScroll)
        store.saveRemoteOverride("902488771", null)
        assertNull(store.remoteOverride("902488771"))
        val saved = RemoteOptions(inputMode = "touch", quality = "best", dragLock = true, customFps = 90, scrollSpeed = 2f).toJson().toString()
        val back = RemoteOptions.fromJson(saved)
        assertEquals("touch", back.inputMode)
        assertEquals("best", back.quality)
        assertTrue(back.dragLock)
        assertEquals(60, back.customFps)
        assertEquals(2f, back.scrollSpeed, 0.01f)
        assertEquals(false, RemoteOptions.fromJson("").dragLock)
    }
}
