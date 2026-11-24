package app.justyo.android

import android.content.Context
import uniffi.yo.SyncedItem

@Suppress("UNUSED_PARAMETER")
internal class SyncedAccount(context: Context) {
    fun read(): SyncedItem? = null
    fun write(item: SyncedItem, wanted: Boolean) = false
    fun missing(): CloudMissing? = null
    fun delete() {}
}
