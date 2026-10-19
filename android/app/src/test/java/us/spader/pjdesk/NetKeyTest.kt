package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.core.CloudAuthException
import us.spader.pjdesk.core.NetKey
import java.util.Base64

class NetKeyTest {
    private val pass =
        "pjdesk-net1:AQEiIiIiIiIiIiIiIiIiIiIiIAAAAAIAAAABAAAAMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzKOc4nuM-OveRpTDfJiHVaAyTTbmwtI0GPMF5rThrZ4SUnHdPSug_sIdPwyd6oaTP-Z_rMTTJ7bPn7zSdR43Sg806UBK7suIiEg8hBFnJ5xnojow6LryvM9RClVImNRq7Uv7DQDkfIn3SqQiwyHFT6H5MXb2SasEGO3nD6HdUmM-CEeMwDeZ6AAAzA5I9qWNMQ54vLg"
    private val embedded =
        "pjdesk-net1:AQJERERERERERERERERERERERERERERERERERERERERERDMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzM5JcfFHZpwBQRaG9g6f0eGoXUeLUeS61xXWt-uI1ycZSw82datp23gtlU1GdZHD-RGiUR0yNWepB_aH8oJQVwDOASUGPNShWH7F0Vqvtue_nYigyTpqI0V-LghRmeC-0bWVub1AHt3nVAyS4RVxgBGUQ1h_kvBuMMV9wDoXNBodr32qLeWJxV4t6xVHgZv4Ne4lf70E"

    @Test
    fun roundTripMatchesTheSharedVector() {
        assertFalse(pass.contains("office.example"))
        val opened = NetKey.open(pass, "correct horse")
        assertEquals("office.example:21200", opened.active)
        assertEquals("abcd", opened.activeFingerprint)
        assertEquals("cabin.example:21200", opened.standby)
        assertEquals("net_1", opened.networkId)
        val alone = NetKey.open(embedded, "")
        assertEquals("office.example:21200", alone.active)
        assertEquals("embedded", NetKey.kind(embedded))
    }

    @Test
    fun filenameAndClipboardKeepTheEncryptedBlob() {
        assertFalse(embedded.contains("office.example"))
        val name = "PJDesk-Setup-0.6.0--$embedded.exe"
        assertTrue(name.length > 255)
        assertEquals(embedded, NetKey.blobIn(name))
        assertEquals(embedded, NetKey.blobIn("C:\\Users\\Ben\\Downloads\\PJDesk-Setup-0.6.0--$embedded (1).exe"))
        assertEquals(embedded, NetKey.blobIn("\n$embedded \n"))
        assertEquals(null, NetKey.blobIn("PJDesk-Setup-0.6.0--host=office.example,key=abc.exe"))
    }

    @Test
    fun wrongPassphraseAndTamperedBlobDoNotOpen() {
        try {
            NetKey.open(pass, "wrong passphrase")
            throw AssertionError("wrong passphrase opened")
        } catch (e: CloudAuthException) {
            assertEquals("That network key could not be opened. Check the text and the passphrase.", e.message)
        }
        val raw = Base64.getUrlDecoder().decode(pass.removePrefix("pjdesk-net1:"))
        raw[raw.size - 1] = (raw[raw.size - 1].toInt() xor 0x01).toByte()
        val tampered = "pjdesk-net1:" + Base64.getUrlEncoder().withoutPadding().encodeToString(raw)
        try {
            NetKey.open(tampered, "correct horse")
            throw AssertionError("tampered blob opened")
        } catch (_: CloudAuthException) {
        }
    }
}
