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
import androidx.core.net.toUri
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext
import uniffi.yo.MessageKind

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
    fun show(thread: String, id: String, name: String?, group: String?, kind: MessageKind) {
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
        val children = manager.activeNotifications.filter {
            it.notification.group == thread && it.notification.flags and Notification.FLAG_GROUP_SUMMARY == 0
        }
        if (children.any { it.tag == id }) return
        val text = if (name == null) context.getString(R.string.yo_notification_fallback) else buildString {
            append(context.getString(R.string.yo_from, "Yo", name))
            if (group != null) append(context.getString(R.string.yo_notification_group_suffix, group.removePrefix("#")))
            when (kind) {
                is MessageKind.Photo -> append(" 📷")
                is MessageKind.Video -> append(" 🎥")
                else -> Unit
            }
        }
        val open = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pending = PendingIntent.getActivity(context, 0, open, PendingIntent.FLAG_IMMUTABLE)
        fun notification(channel: String) = Notification.Builder(context, channel)
            .setSmallIcon(R.drawable.ic_yo_monochrome)
            .setContentIntent(pending)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
            .setGroup(thread)
            .setGroupAlertBehavior(Notification.GROUP_ALERT_CHILDREN)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
        manager.notify(id, 1, notification(if (children.isEmpty()) alert else silent).setContentTitle(text).build())
        manager.notify("summary:$thread", 1, notification(silent).setGroupSummary(true)
            .setContentTitle(text).setNumber(children.size + 1).build())
    }

    fun clear() = manager.cancelAll()
}
