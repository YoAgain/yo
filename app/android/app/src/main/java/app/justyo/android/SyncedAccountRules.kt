package app.justyo.android

import uniffi.yo.SyncedItem

/** Why Google's cloud gets no copy of the account item: no screen lock, else Google's device backup is off. */
enum class CloudMissing { SCREEN_LOCK, DEVICE_BACKUP }

internal object SyncedAccountRules {
    const val KEY = "yo.account"

    /** Google's cloud copy when the person wants it and only with end-to-end encryption assured; never a plain one. */
    fun toCloud(wanted: Boolean, endToEnd: Boolean) = wanted && endToEnd

    /** The automatic sign-in from an item: not signed out, not replaced on this phone, nothing left unmarked. */
    fun signsIn(item: SyncedItem?, left: ByteArray?, unmarked: Boolean): Boolean =
        item != null && item.signedOut != true && !unmarked && !(left != null && left.contentEquals(item.userId))
}
