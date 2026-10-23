package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import us.spader.pjdesk.media.AnnexB

class AnnexBTest {
    private fun sampleConfig() = byteArrayOf(
        0x01, 0x42, 0x00, 0x1E, 0xFF.toByte(),
        0xE1.toByte(), 0x00, 0x03, 0x67, 0x42, 0x00,
        0x01, 0x00, 0x02, 0x68, 0xCE.toByte(),
    )

    @Test
    fun avcCBecomesStartCodes() {
        val csd = sampleConfig()
        val annex = AnnexB.parameterSets(csd)
        assertTrue(AnnexB.isAnnexB(annex))
        assertEquals(7, AnnexB.nalType(annex))
        assertTrue(annex.contains(0x68.toByte()))
    }

    @Test
    fun lengthPrefixedFrameBecomesAnnexBAndAKeyframeRepeatsSps() {
        val converter = AnnexB.Converter()
        converter.config(sampleConfig())
        val frame = byteArrayOf(0, 0, 0, 4, 0x65, 1, 2, 3)
        val key = converter.frame(frame, keyframe = true)
        assertEquals(7, AnnexB.nalType(key))
        assertTrue(key.copyOfRange(key.size - 4, key.size).contentEquals(byteArrayOf(0x65, 1, 2, 3)))
        val delta = converter.frame(byteArrayOf(0, 0, 0, 4, 0x41, 1, 2, 3), keyframe = false)
        assertFalse(AnnexB.nalType(delta) == 7)
        assertEquals(1, AnnexB.nalType(delta))
    }

    @Test
    fun anIdrWithoutTheKeyframeFlagStillRepeatsSps() {
        val converter = AnnexB.Converter()
        converter.config(sampleConfig())
        val idr = byteArrayOf(0, 0, 0, 1, 0x65, 1, 2, 3)
        val out = converter.frame(idr, keyframe = false)
        assertEquals(7, AnnexB.nalType(out))
        assertTrue(AnnexB.containsNal(out, AnnexB.IDR))
        assertTrue(AnnexB.containsNal(out, AnnexB.SPS))
    }

    @Test
    fun aKeyframeThatAlreadyHasSpsIsNotDoubled() {
        val converter = AnnexB.Converter()
        converter.config(sampleConfig())
        val sets = AnnexB.parameterSets(sampleConfig())
        val idr = byteArrayOf(0, 0, 0, 1, 0x65, 9)
        val already = sets + idr
        assertTrue(converter.frame(already, keyframe = true).contentEquals(already))
    }

    @Test
    fun anAccessUnitThatStartsWithAudStillGetsSps() {
        val converter = AnnexB.Converter()
        converter.config(sampleConfig())
        val audThenIdr = byteArrayOf(0, 0, 0, 1, 0x09, 0x10, 0, 0, 0, 1, 0x65, 1)
        val out = converter.frame(audThenIdr, keyframe = true)
        assertEquals(7, AnnexB.nalType(out))
        assertTrue(AnnexB.containsNal(out, AnnexB.IDR))
    }

    @Test
    fun annexBPassesThrough() {
        val data = byteArrayOf(0, 0, 0, 1, 0x65, 9)
        assertTrue(AnnexB.avccToAnnexB(data).contentEquals(data))
        val converter = AnnexB.Converter()
        assertTrue(converter.frame(data, keyframe = true).contentEquals(data))
    }
}
