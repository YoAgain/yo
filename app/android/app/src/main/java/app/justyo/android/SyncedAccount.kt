package app.justyo.android

import android.app.KeyguardManager
import android.content.Context
import com.google.android.gms.auth.blockstore.Blockstore
import com.google.android.gms.auth.blockstore.DeleteBytesRequest
import com.google.android.gms.auth.blockstore.RetrieveBytesRequest
import com.google.android.gms.auth.blockstore.StoreBytesData
import com.google.android.gms.tasks.Task
import com.google.android.gms.tasks.Tasks
import java.util.concurrent.TimeUnit
import uniffi.yo.SyncedItem
import uniffi.yo.syncedItemDecode
import uniffi.yo.syncedItemEncode
import uniffi.yo.syncedItemUpgraded

/** Why Google's cloud gets no copy of the account item: no screen lock, else Google's device backup is off. */
enum class CloudMissing { SCREEN_LOCK, DEVICE_BACKUP }

/**
 * The account item in Google Block Store, as iOS keeps it in the iCloud Keychain (format and parsing: the core's
 * `keychain` module): a new phone with the same Google account signs in by itself. Key material goes to Google's cloud
 * only end-to-end encrypted (spec/v1.md, "Key material only in an end-to-end encrypted cloud"): without that
 * assurance the item stays on the phone for a device-to-device transfer only. Callers run on IO.
 *
 * Block Store's cloud copy travels in Google's device backup: `isEndToEndEncryptionAvailable()` is false both without
 * a screen lock and while "Backup by Google" is off (Block Store logs "no backup account found"), so the reason shown
 * comes from the keyguard, not from that answer alone.
 */
internal class SyncedAccount(context: Context) {
    private val client = Blockstore.getClient(context)
    private val keyguard = context.getSystemService(KeyguardManager::class.java)


    private fun <T> Task<T>.result(): T = Tasks.await(this, 10, TimeUnit.SECONDS)

    /** The stored item in today's format; none when there is none. Throws when Block Store did not answer. */
    fun read(): SyncedItem? {
        val response = client.retrieveBytes(RetrieveBytesRequest.Builder().setKeys(listOf(KEY)).build()).result()
        return response.blockstoreDataMap[KEY]?.bytes?.let(::syncedItemDecode)?.let(::syncedItemUpgraded)
    }

    /**
     * Stores the item; true when its copy is in Google's cloud (`wanted`, and end-to-end encrypted, checked each time).
     * Throws when not stored.
     */
    fun write(item: SyncedItem, wanted: Boolean): Boolean {
        val cloud = toCloud(wanted, endToEnd())
        client.storeBytes(StoreBytesData.Builder().setKey(KEY).setBytes(syncedItemEncode(item))
            .setShouldBackupToCloud(cloud).build()).result()
        return cloud
    }

    private fun endToEnd() = runCatching { client.isEndToEndEncryptionAvailable().result() }.getOrDefault(false)

    /** What keeps the cloud copy away right now; none when it can be made. */
    fun missing(): CloudMissing? = when {
        !keyguard.isDeviceSecure -> CloudMissing.SCREEN_LOCK
        endToEnd() -> null
        else -> CloudMissing.DEVICE_BACKUP
    }

    /** Throws when Block Store did not take it. */
    fun delete() {
        client.deleteBytes(DeleteBytesRequest.Builder().setKeys(listOf(KEY)).build()).result()
    }

    companion object {
        const val KEY = "yo.account"

        /** Google's cloud copy when the person wants it and only with end-to-end encryption assured; never a plain one. */
        fun toCloud(wanted: Boolean, endToEnd: Boolean) = wanted && endToEnd

        /** The automatic sign-in from an item: not signed out, not replaced on this phone, nothing left unmarked. */
        fun signsIn(item: SyncedItem?, left: ByteArray?, unmarked: Boolean): Boolean =
            item != null && item.signedOut != true && !unmarked && !(left != null && left.contentEquals(item.userId))
    }
}
