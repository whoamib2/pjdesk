package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.media.AnnexB

class AnnexBTest {
    @Test
    fun avcCBecomesStartCodes() {
        val csd = byteArrayOf(
            0x01, 0x42, 0x00, 0x1E, 0xFF.toByte(),
            0xE1.toByte(), 0x00, 0x03, 0x67, 0x42, 0x00,
            0x01, 0x00, 0x02, 0x68, 0xCE.toByte(),
        )
        val annex = AnnexB.parameterSets(csd)
        assertTrue(AnnexB.isAnnexB(annex))
        assertEquals(7, AnnexB.nalType(annex))
        assertTrue(annex.contains(0x68.toByte()))
    }

    @Test
    fun lengthPrefixedFrameBecomesAnnexBAndAKeyframeRepeatsSps() {
        val converter = AnnexB.Converter()
        converter.config(
            byteArrayOf(
                0x01, 0x42, 0x00, 0x1E, 0xFF.toByte(),
                0xE1.toByte(), 0x00, 0x03, 0x67, 0x42, 0x00,
                0x01, 0x00, 0x02, 0x68, 0xCE.toByte(),
            ),
        )
        val frame = byteArrayOf(0, 0, 0, 4, 0x65, 1, 2, 3)
        val key = converter.frame(frame, keyframe = true)
        assertEquals(7, AnnexB.nalType(key))
        assertTrue(key.copyOfRange(key.size - 4, key.size).contentEquals(byteArrayOf(0x65, 1, 2, 3)))
        val delta = converter.frame(frame, keyframe = false)
        assertFalse(AnnexB.nalType(delta) == 7)
        assertEquals(5, AnnexB.nalType(delta))
    }

    @Test
    fun annexBPassesThrough() {
        val data = byteArrayOf(0, 0, 0, 1, 0x65, 9)
        assertTrue(AnnexB.avccToAnnexB(data).contentEquals(data))
        val converter = AnnexB.Converter()
        assertTrue(converter.frame(data, keyframe = true).contentEquals(data))
    }
}
