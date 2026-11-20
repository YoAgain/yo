package app.justyo.android

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.provider.Settings
import android.widget.Toast
import androidx.core.app.NotificationCompat
import androidx.core.app.Person
import androidx.core.net.toUri
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext
import uniffi.yo.ListKind
import uniffi.yo.MessageKind
import uniffi.yo.ProductText
import uniffi.yo.productAndroidNotificationLine
import uniffi.yo.productNotificationConversationLine

@Composable
internal fun contextNotificationsEnabled(): Boolean =
    LocalContext.current.getSystemService(NotificationManager::class.java).areNotificationsEnabled()

internal fun openNotificationSettings(context: Context) {
    val settings = Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
        .putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
    try {
        context.startActivity(settings)
    } catch (_: ActivityNotFoundException) {
        val appDetails = Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, "package:${context.packageName}".toUri())
        try {
            context.startActivity(appDetails)
        } catch (_: ActivityNotFoundException) {
            Toast.makeText(context, R.string.notification_settings_unavailable, Toast.LENGTH_SHORT).show()
        }
    }
}

internal class YoNotifications(private val context: Context) {
    private val manager = context.getSystemService(NotificationManager::class.java)
    private val alert = "yo_alerts"
    private val silent = "yo_followups"

    @SuppressLint("MissingPermission")
    /** [friend] and [account]: the bar the Yo landed on, for the notification's "Yo Back" (as iOS's action). */
    fun show(thread: String, id: String, name: String?, group: String?, kind: MessageKind, friend: String? = null, account: String? = null) {
        if (!manager.areNotificationsEnabled()) return
        manager.createNotificationChannels(listOf(
            NotificationChannel(alert, context.getString(R.string.yo_notification_channel), NotificationManager.IMPORTANCE_DEFAULT).apply {
                enableVibration(true)
                setSound("android.resource://${context.packageName}/raw/yo".toUri(),
                    AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_NOTIFICATION).build())
            },
            NotificationChannel(silent, context.getString(R.string.yo_followup_channel), NotificationManager.IMPORTANCE_LOW),
        ))
        if (manager.getNotificationChannel(alert)?.importance == NotificationManager.IMPORTANCE_NONE) return
        // One notification per conversation, updated in place with each Yo (MessagingStyle, as Android's messaging
        // guidance has it); the conversation's line counts them: "5× Yo from Leo · 1 photo" (the core's line).
        val tag = "$CONVERSATION$thread"
        val shown = manager.activeNotifications.firstOrNull { it.tag == tag }?.notification
        val ids = shown?.extras?.getStringArray(IDS).orEmpty()
        if (id in ids) return
        val listKind = when (kind) {
            is MessageKind.Photo -> ListKind.PHOTO
            is MessageKind.Video -> ListKind.VIDEO
            else -> ListKind.YO
        }
        val kinds = (shown?.extras?.getIntArray(KINDS) ?: IntArray(0)).map { ListKind.entries.getOrElse(it) { ListKind.YO } } + listKind
        val line = if (name == null && group == null) productAndroidNotificationLine(null, null, listKind, Words)
            else productNotificationConversationLine(kinds, name ?: copy(ProductText.SOMEONE), group, false, Words)
        val style = shown?.let { NotificationCompat.MessagingStyle.extractMessagingStyleFromNotification(it) }
            ?: NotificationCompat.MessagingStyle(Person.Builder().setName(copy(ProductText.YO)).build())
        style.setConversationTitle(line).setGroupConversation(true)
        val sign = when (listKind) { ListKind.PHOTO -> " 📷"; ListKind.VIDEO -> " 🎥"; else -> "" }
        style.addMessage(copy(ProductText.YO) + sign, System.currentTimeMillis(),
            Person.Builder().setName(name ?: copy(ProductText.SOMEONE)).build())
        val open = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pending = PendingIntent.getActivity(context, 0, open, PendingIntent.FLAG_IMMUTABLE)
        val notification = NotificationCompat.Builder(context, alert)
            .setSmallIcon(R.drawable.ic_yo_monochrome)
            .setStyle(style)
            .setContentTitle(line)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setContentIntent(pending)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)  // the first Yo of a conversation sounds, the next update it quietly
            .setNumber(kinds.size)
            .setGroup(GROUP)
            .setGroupAlertBehavior(NotificationCompat.GROUP_ALERT_CHILDREN)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .addExtras(android.os.Bundle().apply {
                putStringArray(IDS, (ids.toList() + id).takeLast(500).toTypedArray())
                putIntArray(KINDS, kinds.map { it.ordinal }.toIntArray())
                putString(LINE, line)
            })
        if (friend != null && account != null) {
            val back = Intent(context, YoBackReceiver::class.java).setAction(YoBackReceiver.ACTION)
                .putExtra("friend", friend).putExtra("account", account).putExtra("thread", thread)
            val action = PendingIntent.getBroadcast(context, thread.hashCode(), back, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
            notification.addAction(NotificationCompat.Action.Builder(null, context.getString(R.string.yo_back_action), action).build())
        }
        manager.notify(tag, 1, notification.build())
        summarize()
    }

    /** With two conversations or more, their group's summary (Android bundles them under it). */
    @SuppressLint("MissingPermission")
    private fun summarize() {
        val conversations = manager.activeNotifications.filter { it.tag?.startsWith(CONVERSATION) == true }
        if (conversations.size < 2) return manager.cancel(SUMMARY, 1)
        val lines = NotificationCompat.InboxStyle()
        conversations.forEach { lines.addLine(it.notification.extras.getString(LINE)) }
        manager.notify(SUMMARY, 1, NotificationCompat.Builder(context, silent)
            .setSmallIcon(R.drawable.ic_yo_monochrome)
            .setStyle(lines)
            .setContentIntent(PendingIntent.getActivity(context, 0,
                Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP), PendingIntent.FLAG_IMMUTABLE))
            .setAutoCancel(true)
            .setNumber(conversations.sumOf { it.notification.extras.getStringArray(IDS)?.size ?: 1 })
            .setGroup(GROUP)
            .setGroupSummary(true)
            .setGroupAlertBehavior(NotificationCompat.GROUP_ALERT_CHILDREN)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .build())
    }

    private companion object {
        const val CONVERSATION = "yo:"
        const val SUMMARY = "yo-summary"
        const val GROUP = "yo"
        const val IDS = "yo.ids"
        const val KINDS = "yo.kinds"
        const val LINE = "yo.line"
    }

    fun clear() = manager.cancelAll()
    /** A thread's notifications, once its Yo Back went. */
    fun clear(thread: String) {
        manager.cancel("$CONVERSATION$thread", 1)
        summarize()
    }
}

/** A notification's "Yo Back": a Yo to the bar it came from, sent in the background like the tile's. */
class YoBackReceiver : android.content.BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION) return
        val friend = intent.getStringExtra("friend") ?: return
        YoWork.enqueue(context, "send", friend = friend, account = intent.getStringExtra("account"), expedited = true)
        intent.getStringExtra("thread")?.let { YoNotifications(context).clear(it) }
    }
    companion object { const val ACTION = "app.justyo.android.YO_BACK" }
}
