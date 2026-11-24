package app.justyo.android

import android.app.Application
import android.content.Context
import com.google.android.gms.tasks.Tasks
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import com.google.firebase.FirebaseOptions
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

internal object Push {
    fun init(app: Application) {
        if (FirebaseOptions.fromResource(app) != null) FirebaseApp.initializeApp(app)
    }

    fun register(context: Context) {
        if (FirebaseApp.getApps(context).isEmpty()) return
        FirebaseMessaging.getInstance().isAutoInitEnabled = true
        YoWork.enqueue(context, "token")
    }

    fun unregister(context: Context) {}

    fun token(context: Context, renew: Boolean = false): String? {
        if (FirebaseApp.getApps(context).isEmpty()) {
            if (renew) error("no Firebase in this build")
            return null
        }
        val messaging = FirebaseMessaging.getInstance()
        // Renew (Settings › Status, as Signal's): a token Google Play services will not delete now (SERVICE_NOT_AVAILABLE,
        // on a Galaxy A34 every time, while pushes came) does not stop it: the token Firebase has goes to the hub again.
        if (renew) try { Tasks.await(messaging.deleteToken(), 20, TimeUnit.SECONDS) }
        catch (_: ExecutionException) {} catch (_: TimeoutException) {}
        return Tasks.await(messaging.token, 20, TimeUnit.SECONDS)
    }
}

class YoMessagingService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        YoWork.enqueue(this, "token")
    }

    override fun onDeletedMessages() {
        YoWork.enqueue(this, "inbox")
    }

    override fun onMessageReceived(message: RemoteMessage) {
        if (message.data["yo"] != "1") return
        YoWork.enqueue(this, "inbox", urgent = message.data["urgent"] == "true",
            expedited = message.priority == RemoteMessage.PRIORITY_HIGH)
    }
}
