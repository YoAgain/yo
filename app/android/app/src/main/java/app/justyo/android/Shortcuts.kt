package app.justyo.android

import android.content.Context
import android.content.Intent
import androidx.core.app.Person
import androidx.core.content.LocusIdCompat
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import uniffi.yo.ProductText

internal const val ACTION_SEND_YO = "app.justyo.android.SEND_YO"
// Only the launcher shortcut can enter MainActivity through this private alias.
internal const val SHORTCUT_ACTIVITY = "app.justyo.android.ShortcutActivity"

internal val Friend.shortcutEligible get() = !blocked && !gone && !request
internal val Friend.shortcutLabel get() = name.ifBlank { copy(ProductText.YO) }.let {
    if (group == null) it else "#${it.removePrefix("#")}"
}

private fun shortcut(context: Context, friend: Friend, rank: Int): ShortcutInfoCompat {
    val label = friend.shortcutLabel
    val icon = Avatars.icon(context, label, friend.color)
    return ShortcutInfoCompat.Builder(context, friend.key)
        .setShortLabel(label).setLongLabel(label).setIcon(icon)
        .setPerson(Person.Builder().setKey(friend.target).setName(label).setIcon(icon).build())
        .setLongLived(true).setLocusId(LocusIdCompat(friend.key))
        .setCategories(setOf("app.justyo.android.SHARE", "android.shortcut.conversation"))
        .setRank(rank)
        .setIntent(Intent(ACTION_SEND_YO).setClassName(context, SHORTCUT_ACTIVITY).putExtra("friend", friend.key))
        .build()
}

/** Keep eligible conversations cached even when they fall out of the launcher's ranked list. */
internal fun syncShortcuts(context: Context, friends: List<Friend>) {
    pushed.clear()
    runCatching {
        val flags = ShortcutManagerCompat.FLAG_MATCH_DYNAMIC or ShortcutManagerCompat.FLAG_MATCH_CACHED
        val previous = ShortcutManagerCompat.getShortcuts(context, flags).map { it.id }
        val eligible = friends.filter { it.shortcutEligible }
        val keys = eligible.mapTo(mutableSetOf()) { it.key }
        val expired = previous.filter { it !in keys }
        try {
            if (eligible.isEmpty()) {
                ShortcutManagerCompat.removeAllDynamicShortcuts(context)
                return
            }
            val limit = (ShortcutManagerCompat.getMaxShortcutCountPerActivity(context) - 2).coerceIn(0, 6)
            ShortcutManagerCompat.setDynamicShortcuts(context, eligible.take(limit).mapIndexed { index, friend ->
                shortcut(context, friend, index)
            })
        } finally {
            if (expired.isNotEmpty()) ShortcutManagerCompat.removeLongLivedShortcuts(context, expired)
        }
    }
}

/** The conversation shortcuts this process pushed in the last minute, as pushed: Yos from the same friend in a row push
 *  it once (as Signal pushes only if needed). A push is a call to the system with an icon, ~20 ms of a notification;
 *  after a minute it goes again, in case the system dropped it for others meanwhile. */
private val pushed = java.util.concurrent.ConcurrentHashMap<String, Pair<Triple<String, Int, Int>, Long>>()

internal fun pushConversation(context: Context, friend: Friend, rank: Int) {
    val shortcut = Triple(friend.shortcutLabel, friend.color, rank)
    val now = android.os.SystemClock.elapsedRealtime()
    if (pushed[friend.key]?.let { (pushedAs, at) -> pushedAs == shortcut && now - at < 60_000 } == true) return
    runCatching { ShortcutManagerCompat.pushDynamicShortcut(context, shortcut(context, friend, rank)) }
        .onSuccess { pushed[friend.key] = shortcut to now }
}

internal fun reportShortcutUsed(context: Context, friend: Friend) {
    runCatching { ShortcutManagerCompat.reportShortcutUsed(context, friend.key) }
}
