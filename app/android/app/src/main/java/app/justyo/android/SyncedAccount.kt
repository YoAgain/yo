package app.justyo.android

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

/**
 * The account item in Google Block Store, as iOS keeps it in the iCloud Keychain (format and parsing: the core's
 * `keychain` module): a new phone with the same Google account signs in by itself. Block Store needs Google Play
 * services; its cloud copy is end-to-end encrypted only while the phone has a screen lock. Callers run on IO.
 */
internal class SyncedAccount(context: Context) {
    private val client = Blockstore.getClient(context)

    private fun <T> Task<T>.result(): T = Tasks.await(this, 10, TimeUnit.SECONDS)

    /** The stored item in today's format; none without one, without Play services, or on any failure. */
    fun read(): SyncedItem? = runCatching {
        val response = client.retrieveBytes(RetrieveBytesRequest.Builder().setKeys(listOf(KEY)).build()).result()
        response.blockstoreDataMap[KEY]?.bytes?.let(::syncedItemDecode)?.let(::syncedItemUpgraded)
    }.getOrNull()

    fun write(item: SyncedItem) {
        runCatching {
            client.storeBytes(StoreBytesData.Builder().setKey(KEY).setBytes(syncedItemEncode(item))
                .setShouldBackupToCloud(true).build()).result()
        }
    }

    /** Deleted with the account; signing out only marks it (`signedOut`), as on iOS. */
    fun delete() {
        runCatching { client.deleteBytes(DeleteBytesRequest.Builder().setKeys(listOf(KEY)).build()).result() }
    }

    companion object {
        const val KEY = "yo.account"
    }
}
