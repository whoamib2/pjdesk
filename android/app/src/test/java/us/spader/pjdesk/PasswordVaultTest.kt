package us.spader.pjdesk

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import us.spader.pjdesk.core.AccountPin
import us.spader.pjdesk.core.AppLockPolicy
import us.spader.pjdesk.core.PasswordVault
import us.spader.pjdesk.core.PinWrap
import us.spader.pjdesk.core.VaultEntry

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [33])
class PasswordVaultTest {
    @Test
    fun sealRoundTrips() {
        val key = ByteArray(32) { 7 }
        val nonce = ByteArray(12) { 3 }
        val blob = PasswordVault.sealWithNonce(key, listOf(VaultEntry("902488771", "cabin", 10)), nonce)
        assertTrue(blob.startsWith("UEpWQVVMVD"))
        assertEquals(listOf(VaultEntry("902488771", "cabin", 10)), PasswordVault.open(key, blob))
    }

    @Test
    fun mergeMatchesTheAccountCopy() {
        val onlyLocal = PasswordVault.merge(listOf(VaultEntry("902488771", "cabin", 0)), emptyList(), 50)
        assertTrue(onlyLocal.remoteChanged)
        assertEquals(VaultEntry("902488771", "cabin", 50), onlyLocal.entries.single())

        val fromAccount = PasswordVault.merge(emptyList(), listOf(VaultEntry("902488771", "cabin", 10)), 50)
        assertTrue(fromAccount.localChanged)
        assertFalse(fromAccount.remoteChanged)

        val newer = PasswordVault.merge(
            listOf(VaultEntry("902488771", "old", 5)),
            listOf(VaultEntry("902488771", "new", 9)),
            50,
        )
        assertEquals("new", newer.entries.single().password)
        assertTrue(newer.localChanged)

        val deleted = PasswordVault.merge(
            listOf(VaultEntry("902488771", "cabin", 4)),
            listOf(VaultEntry("902488771", "", 9)),
            50,
        )
        assertEquals("", deleted.entries.single().password)
    }

    @Test
    fun pinWrapRejectsTheWrongPin() {
        val salt = ByteArray(16) { 1 }
        val key = ByteArray(32) { 9 }
        val stored = PinWrap.wrap(key, "1234", salt)
        assertTrue(PinWrap.unwrap(stored, "1234", salt)!!.contentEquals(key))
        assertEquals(null, PinWrap.unwrap(stored, "9999", salt))
    }

    @Test
    fun lockPolicy() {
        assertTrue(AppLockPolicy.pinAcceptable("1234"))
        assertTrue(AppLockPolicy.pinAcceptable("12345678"))
        assertFalse(AppLockPolicy.pinAcceptable("123"))
        assertFalse(AppLockPolicy.pinAcceptable("12345a"))
        assertFalse(AppLockPolicy.lockedOut(4))
        assertTrue(AppLockPolicy.lockedOut(5))
        assertFalse(AppLockPolicy.relock(30_000, 120_000))
        assertTrue(AppLockPolicy.relock(120_000, 120_000))
        assertEquals(0, AppLockPolicy.lockoutSeconds(4))
        assertEquals(60, AppLockPolicy.lockoutSeconds(5))
        assertEquals(120, AppLockPolicy.lockoutSeconds(6))
        assertEquals(1800, AppLockPolicy.lockoutSeconds(9))
    }

    @Test
    fun accountPinMatchesTheSharedHashAndRoundTrips() {
        val salt = ByteArray(16) { 9 }
        val hash = AccountPin.hashPin("123456", salt, 1000)
        assertEquals("c5a49ff1b52892a67356c2d284d98a7b145b9eedfada7621e21b8befa71efa97", hash.joinToString("") { "%02x".format(it) })
        val slow = AccountPin.hashPin("123456", salt, AccountPin.ITERATIONS)
        assertEquals("a7d083bc95f7969cfc2f3d2d32a1cf3622ed8891720a419d127451f882239fe3", slow.joinToString("") { "%02x".format(it) })
        val key = ByteArray(32) { 4 }
        val made = AccountPin.create("1234", 0, 0)
        assertEquals(AccountPin.IDLE_DEFAULT, made.idleSec)
        assertTrue(AccountPin.matches("1234", made.salt, made.hash, made.iterations))
        assertFalse(AccountPin.matches("9999", made.salt, made.hash, made.iterations))
        val sealed = AccountPin.seal(key, made)
        assertTrue(sealed.startsWith("UEpMT0NLMD"))
        val opened = AccountPin.open(key, sealed)
        assertEquals(made.hash, opened.hash)
        assertTrue(AccountPin.matches("1234", opened.salt, opened.hash, opened.iterations))
    }
}
