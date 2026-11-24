package app.justyo.android

import android.app.Application
import android.app.ForegroundServiceStartNotAllowedException
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

@Suppress("UNUSED_PARAMETER")
internal object Push {
    fun init(app: Application) {}
    fun token(context: Context, renew: Boolean = false): String? = null

    fun register(context: Context) {
        try { context.startForegroundService(Intent(context, YoConnectionService::class.java)) }
        catch (_: ForegroundServiceStartNotAllowedException) {} // Left during sign-in. Retry on foreground.
    }

    fun unregister(context: Context) {
        context.stopService(Intent(context, YoConnectionService::class.java))
    }
}

class ConnectionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == Intent.ACTION_BOOT_COMPLETED || intent.action == Intent.ACTION_MY_PACKAGE_REPLACED)
            Push.register(context)
    }
}

class YoConnectionService : Service() {
    private val scope = MainScope()

    override fun onCreate() {
        super.onCreate()
        val channel = NotificationChannel("connection", getString(R.string.connection_channel), NotificationManager.IMPORTANCE_LOW)
        channel.setSound(null, null)
        channel.setShowBadge(false)
        getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        val notification = Notification.Builder(this, channel.id)
            .setSmallIcon(R.drawable.ic_yo_monochrome)
            .setContentTitle(getString(R.string.connection_channel))
            .setContentText(getString(R.string.connection_notification))
            .setContentIntent(open).setOngoing(true).build()
        startForeground(2, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        scope.launch {
            val state = repository.state.first { !it.loading }
            if (state.username == null || state.broken) stopSelf() else repository.keepConnection(true)
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int) = START_STICKY
    override fun onBind(intent: Intent?): IBinder? = null
    override fun onDestroy() {
        scope.cancel()
        repository.keepConnection(false)
        super.onDestroy()
    }
}
