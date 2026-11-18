package app.justyo.android

import android.content.res.Resources
import uniffi.yo.ListRow

/** Core labels remain protocol values; only the rendering layer translates them. */
internal fun rowTitle(resources: Resources,row: ListRow?,friend: Friend): String {
    if(friend.request) return resources.getString(if(friend.group==null) R.string.request_from else R.string.group_request,friend.name)
    val title = row?.title ?: friend.name
    if (title == friend.name) return if (friend.group == null) title else "#" + title.removePrefix("#")
    val resource=when(title) {
        "● Recording" -> R.string.recording
        "Not on Yo" -> R.string.not_on_yo
        "Group Is Empty" -> R.string.group_empty
        "Confirm New Key" -> R.string.ui_confirm_new_key
        "Blocked" -> R.string.ui_blocked
        "Needs Update" -> R.string.needs_update
        "Not Sent" -> R.string.not_sent
        "Canceled" -> R.string.ui_canceled
        "Already Added" -> R.string.already_added
        else -> null
    }
    return resource?.let(resources::getString) ?: title
}
