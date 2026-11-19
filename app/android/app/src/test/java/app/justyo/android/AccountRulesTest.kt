package app.justyo.android

import com.google.zxing.BarcodeFormat
import com.google.zxing.ResultMetadataType
import org.junit.Assert.*
import org.junit.Test
import uniffi.yo.*

/** The account's rules on Android: the one-time sign-in, Block Store and the safety code's QR bytes. */
class AccountRulesTest {
    private val key = ByteArray(32) { 7 }

    /** An old install's sign-in is kept as an item (the vault survives the process): key first, else its recovery key. */
    @Test fun anOldInstallKeepsItsSignIn() {
        assertNull(Repository.pendingSignIn(null, LegacyIdentity(key, false, null), "code"))
        assertNull(Repository.pendingSignIn("Mia", null, null))
        // No recovery key: the account key of the old state signs in, never dropped.
        val kept = Repository.pendingSignIn("Mia", LegacyIdentity(key, false, null), null)!!
        val again = syncedItemDecode(syncedItemEncode(kept))!!
        assertArrayEquals(key, again.accountKey)
        assertEquals(syncedItemVersion(), again.version)
        assertNull(again.recoveryKey)
        // A seed of then is converted, its storage key kept.
        val seed = syncedItemUpgraded(Repository.pendingSignIn("Mia", LegacyIdentity(key, true, null), null)!!)!!
        assertArrayEquals(key, seed.legacySeed)
        // Only a recovery key: that restores.
        val code = Repository.pendingSignIn("Mia", null, "code")!!
        assertTrue(code.accountKey.isEmpty())
        assertEquals("code", syncedItemDecode(syncedItemEncode(code))!!.recoveryKey)
    }

    /**
     * A converted (Ed25519) account's hub storage key outlives the vault's clearing in `migrate` with the kept sign-in,
     * so `finishMigration` opens its storage (friends, groups) with it. A turned-off Google backup outlives it too.
     */
    @Test fun aConvertedAccountKeepsItsStorageKeyThroughTheUpdate() {
        for (name in listOf("migration", "migration-storage", "migration-storage.bak", "state-key", "google-backup")) assertTrue(name, Vault.keeps(name))
        for (name in listOf("ui", "recovery", "ui.bak")) assertFalse(name, Vault.keeps(name))
    }

    /** The one-time sign-in is dropped only when the hub refused for good, never for a busy or restarting hub. */
    @Test fun aPendingSignInSurvivesATemporaryHubError() {
        for (status in listOf(500, 502, 503, 429)) assertTrue("$status", Repository.keepsPendingSignIn(YoException.Refused(status.toUShort())))
        assertTrue(Repository.keepsPendingSignIn(YoException.Network("offline")))
        for (status in listOf(401, 404, 409, 410)) assertFalse("$status", Repository.keepsPendingSignIn(YoException.Refused(status.toUShort())))
        assertFalse(Repository.keepsPendingSignIn(IllegalStateException("no recovery key")))
    }

    @Test fun keyMaterialGoesToGooglesCloudOnlyEndToEndEncrypted() {
        assertTrue(SyncedAccount.toCloud(wanted = true, endToEnd = true))
        assertFalse(SyncedAccount.toCloud(wanted = true, endToEnd = false))
        assertFalse(SyncedAccount.toCloud(wanted = false, endToEnd = true))
    }

    /** No automatic sign-in into a signed-out item, the account another phone took, or while a mark waits. */
    @Test fun theAutomaticSignInRespectsSignOutReplacementAndWaitingMarks() {
        val item = SyncedItem("Mia", byteArrayOf(1, 2), key, false, true, true, false, "code", null, syncedItemVersion(), null)
        assertTrue(SyncedAccount.signsIn(item, null, false))
        assertTrue(SyncedAccount.signsIn(item, byteArrayOf(9), false))
        assertFalse(SyncedAccount.signsIn(item.copy(signedOut = true), null, false))
        assertFalse(SyncedAccount.signsIn(item, byteArrayOf(1, 2), false))
        assertFalse(SyncedAccount.signsIn(item, null, true))
        assertFalse(SyncedAccount.signsIn(null, null, false))
    }

    /** A safety code is binary: the scanner hands over the QR code's own bytes, not its text. */
    @Test fun aScannedSafetyCodeKeepsItsBytes() {
        val bytes = byteArrayOf(0x0a, 0x80.toByte(), 0xff.toByte(), 0)
        val result = com.google.zxing.Result("ignored", null, null, BarcodeFormat.QR_CODE)
        result.putMetadata(ResultMetadataType.BYTE_SEGMENTS, listOf(bytes.copyOfRange(0, 2), bytes.copyOfRange(2, 4)))
        assertArrayEquals(bytes, scanned(result).bytes)
        assertEquals("yo", scanned(com.google.zxing.Result("yo", null, null, BarcodeFormat.QR_CODE)).text)
    }
}
