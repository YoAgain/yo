package app.justyo.android

import android.annotation.SuppressLint
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
import androidx.core.app.PendingIntentCompat
import androidx.core.app.Person
import androidx.core.app.RemoteInput
import androidx.core.content.LocusIdCompat
import androidx.core.graphics.drawable.IconCompat
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

    // The channels from the start, not with the first Yo: Yo's notification settings list them (sound, importance)
    // before anything came, and with notifications first turned off they are there once turned on.
    init {
        manager.createNotificationChannels(listOf(
            NotificationChannel(alert, context.getString(R.string.yo_notification_channel), NotificationManager.IMPORTANCE_DEFAULT).apply {
                enableVibration(true)
                setSound("android.resource://${context.packageName}/raw/yo".toUri(),
                    AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_NOTIFICATION).build())
            },
            NotificationChannel(silent, context.getString(R.string.yo_followup_channel), NotificationManager.IMPORTANCE_LOW),
        ))
    }

    /** Each thread keeps its message history until it is opened or cleared. */
    @SuppressLint("MissingPermission")
    fun show(thread: String, id: String, kind: MessageKind, friend: Friend? = null, name: String? = null,
             sender: String? = null, senderColor: Int? = null, account: String? = null, rank: Int = 0) {
        if (!manager.areNotificationsEnabled()) return
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
        // Nobody to name ("You may have received a Yo", the push's fallback among them): the line alone, not a
        // conversation with a made-up "Someone: Yo" in it.
        val group = friend?.takeIf { it.group != null }?.name
        val anonymous = name == null && group == null
        val line = if (anonymous) productAndroidNotificationLine(null, null, listKind, Words)
            else productNotificationConversationLine(kinds, name ?: copy(ProductText.SOMEONE), group, false, Words)
        val open = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pending = PendingIntent.getActivity(context, 0, open, PendingIntent.FLAG_IMMUTABLE)
        val notification = NotificationCompat.Builder(context, alert)
            .setSmallIcon(R.drawable.ic_yo_monochrome)
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
        if (!anonymous && friend != null) {
            val me = Person.Builder().setKey(account).setName(context.getString(R.string.notification_you)).build()
            val style = NotificationCompat.MessagingStyle(me).setGroupConversation(group != null)
            if (group != null) style.setConversationTitle(friend.shortcutLabel)
            shown?.let { NotificationCompat.MessagingStyle.extractMessagingStyleFromNotification(it) }
                ?.messages?.forEach { style.addMessage(it) }
            val senderName = name ?: copy(ProductText.SOMEONE)
            val person = Person.Builder().setKey(sender).setName(senderName)
                .setIcon(Avatars.icon(context, name.orEmpty(), senderColor)).build()
            val sign = when (listKind) { ListKind.PHOTO -> " 📷"; ListKind.VIDEO -> " 🎥"; else -> "" }
            notification.setStyle(style.addMessage(copy(ProductText.YO) + sign, System.currentTimeMillis(), person))
            if (friend.shortcutEligible) {
                pushConversation(context, friend, rank)
                notification.setShortcutId(friend.key).setLocusId(LocusIdCompat(friend.key))
            }
            if (account != null) addActions(notification, thread, friend.key, account)
        }
        manager.notify(tag, 1, notification.build())
        summarize()
    }

    private fun addActions(builder: NotificationCompat.Builder, thread: String, friend: String, account: String) {
        fun broadcast(action: String, mutable: Boolean): PendingIntent? {
            val intent = Intent(context, YoBackReceiver::class.java).setAction(action)
                .setData(android.net.Uri.fromParts("yo-notification", thread, null))
                .putExtra("friend", friend).putExtra("account", account).putExtra("thread", thread)
            return PendingIntentCompat.getBroadcast(context, thread.hashCode(), intent, PendingIntent.FLAG_UPDATE_CURRENT, mutable)
        }
        val back = context.getString(R.string.yo_back_action)
        builder.addAction(NotificationCompat.Action.Builder(IconCompat.createWithResource(context, R.drawable.ic_notification_reply),
            back, broadcast(YoBackReceiver.ACTION, true))
            // A reply that is always a Yo: no text field on the phone (a tap sends it), while Android Auto and Wear still
            // see a reply action they can offer by voice.
            .addRemoteInput(RemoteInput.Builder("yo_reply").setLabel(back).setChoices(arrayOf(copy(ProductText.YO)))
                .setAllowFreeFormInput(false).build())
            .setAllowGeneratedReplies(false).setSemanticAction(NotificationCompat.Action.SEMANTIC_ACTION_REPLY)
            .setShowsUserInterface(false).build())
        builder.addAction(NotificationCompat.Action.Builder(IconCompat.createWithResource(context, R.drawable.ic_notification_read),
            context.getString(R.string.notification_mark_read), broadcast(YoBackReceiver.MARK_READ, false))
            .setSemanticAction(NotificationCompat.Action.SEMANTIC_ACTION_MARK_AS_READ)
            .setShowsUserInterface(false).build())
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
        when (intent.action) {
            ACTION -> {
                val friend = intent.getStringExtra("friend") ?: return
                YoWork.enqueue(context, "send", friend = friend, account = intent.getStringExtra("account"), expedited = true)
            }
            MARK_READ -> Unit
            else -> return
        }
        intent.getStringExtra("thread")?.let { YoNotifications(context).clear(it) }
    }
    companion object {
        const val ACTION = "app.justyo.android.YO_BACK"
        const val MARK_READ = "app.justyo.android.MARK_READ"
    }
}
