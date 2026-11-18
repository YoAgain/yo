package app.justyo.android

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.compose.runtime.Composable
import androidx.compose.ui.platform.LocalContext
import uniffi.yo.MessageKind

@Composable
internal fun contextNotificationsEnabled(): Boolean =
    LocalContext.current.getSystemService(NotificationManager::class.java).areNotificationsEnabled()

/** Updates the sender's existing notification while it is still in the shade. */
internal class YoNotifications(private val context: Context) {
    private val manager = context.getSystemService(NotificationManager::class.java)
    private val channel = "yos"

    // Checked against the system permission and channel before posting.
    @SuppressLint("MissingPermission")
    fun show(sender: String, name: String?, kind: MessageKind) {
        if (!manager.areNotificationsEnabled()) return
        manager.createNotificationChannel(NotificationChannel(
            channel, context.getString(R.string.yo_notification_channel), NotificationManager.IMPORTANCE_LOW,
        ))
        if (manager.getNotificationChannel(channel)?.importance == NotificationManager.IMPORTANCE_NONE) return
        val tag = if (name == null) "unread-yo" else "yo:$sender"
        val previous = manager.activeNotifications.firstOrNull { it.tag == tag }?.notification
        val count = ((previous?.number ?: 0) + 1).coerceAtLeast(1)
        val photo = kind is MessageKind.Photo || previous?.extras?.getBoolean("yo.photo") == true
        val video = kind is MessageKind.Video || previous?.extras?.getBoolean("yo.video") == true
        val text = if (name == null) context.getString(R.string.yo_notification_fallback) else buildString {
            val yos = if (count > 1) context.getString(R.string.counted_yo, count.toLong()) else "Yo"
            append(context.getString(R.string.yo_from, yos, name))
            if (photo) append(" 📷")
            if (video) append(" 🎥")
        }
        val open = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val notification = Notification.Builder(context, channel)
            .setSmallIcon(R.drawable.ic_yo_monochrome)
            .setContentTitle(text)
            .setContentIntent(PendingIntent.getActivity(context, 0, open, PendingIntent.FLAG_IMMUTABLE))
            .setAutoCancel(true)
            .setNumber(count)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .build()
        notification.extras.putBoolean("yo.photo", photo)
        notification.extras.putBoolean("yo.video", video)
        manager.notify(tag, 1, notification)
    }

    fun clear() = manager.cancelAll()
}
