package app.justyo.android

import android.content.Context
import android.content.Intent
import android.content.pm.ShortcutInfo
import android.content.pm.ShortcutManager
import android.graphics.drawable.Icon

/** Native chooser targets. The launcher can only see the same names explicitly shown in this app. */
internal fun updateSharingShortcuts(context: Context,friends: List<Friend>) {
    val manager=context.getSystemService(ShortcutManager::class.java)
    val shortcuts=friends.filter { !it.blocked && !it.gone && !it.request }.take(manager.maxShortcutCountPerActivity.coerceAtMost(4)).map { friend ->
        // A group as its bar says it: "#HUNT" (it was "HUNT" in the share sheet).
        val label=friend.name.ifBlank { "Yo" }.let { if(friend.group!=null) "#${it.removePrefix("#")}" else it }
        ShortcutInfo.Builder(context,friend.key).setShortLabel(label)
            .setIcon(Icon.createWithResource(context,R.mipmap.launcher_standard))
            .setCategories(setOf("app.justyo.android.SHARE"))
            .setIntent(Intent(context,MainActivity::class.java).setAction(Intent.ACTION_MAIN))
            .apply { if(android.os.Build.VERSION.SDK_INT>=29) setLongLived(true) }.build()
    }
    if(android.os.Build.VERSION.SDK_INT>=30) {
        val current=shortcuts.map { it.id }.toSet()
        val expired=manager.getShortcuts(ShortcutManager.FLAG_MATCH_DYNAMIC or ShortcutManager.FLAG_MATCH_CACHED).map { it.id }.filter { it !in current }
        if(expired.isNotEmpty()) manager.removeLongLivedShortcuts(expired)
    }
    manager.dynamicShortcuts=shortcuts
}
