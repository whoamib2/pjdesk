package us.spader.pjdesk

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import us.spader.pjdesk.core.CloudAuth
import us.spader.pjdesk.core.SavedComputer

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [33])
class CloudAuthTest {
    @Test
    fun canonicalMatchesTheSharedFixture() {
        val text = CloudAuth.canonical(
            "Ben@Spader.us",
            "net_1",
            listOf("AA:BB:CC:DD", "aabbccdd", "00112233"),
            "device-1",
            2000000000L,
        )
        assertEquals("PJN1\nben@spader.us\nnet_1\n00112233,aabbccdd\ndevice-1\n2000000000\n", text)
    }

    @Test
    fun savedComputersCarryNoPassword() {
        val array = CloudAuth.computersJson(listOf(SavedComputer("123456789", "Office", "windows", "11", true)))
        val text = array.toString()
        assertFalse(text.contains("password"))
        assertEquals("Office", JSONObject(text.removePrefix("[").removeSuffix("]")).getString("name"))
    }

    @Test
    fun authKeyIsStableAndNotThePassword() {
        val key = CloudAuth.authKeyHex("ben@spader.us", "correct horse battery")
        assertEquals(64, key.length)
        assertEquals(key, CloudAuth.authKeyHex("Ben@Spader.us", "correct horse battery"))
        assertFalse(key.contains("horse"))
    }
}
