package app.justyo.android

import android.app.Application
import android.content.Context
import android.os.Build
import android.os.SystemClock
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraManager
import androidx.camera.camera2.Camera2Config
import androidx.camera.core.CameraSelector
import androidx.camera.core.CameraXConfig
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import com.google.android.gms.tasks.Tasks
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.util.concurrent.TimeUnit
import uniffi.yo.YoException

class YoApplication : Application(), CameraXConfig.Provider {
    override fun getCameraXConfig(): CameraXConfig {
        val default = Camera2Config.defaultConfig()
        val manager = getSystemService(CameraManager::class.java)
        // Some devices advertise both camera features but expose only one facing.
        val facing = runCatching {
            manager.cameraIdList.mapNotNull {
                manager.getCameraCharacteristics(it).get(CameraCharacteristics.LENS_FACING)
            }.distinct().singleOrNull()
        }.getOrNull()
        val limiter = when (facing) {
            CameraCharacteristics.LENS_FACING_FRONT -> CameraSelector.DEFAULT_FRONT_CAMERA
            CameraCharacteristics.LENS_FACING_BACK -> CameraSelector.DEFAULT_BACK_CAMERA
            else -> return default
        }
        return CameraXConfig.Builder.fromConfig(default).setAvailableCamerasLimiter(limiter).build()
    }

    override fun onCreate() {
        super.onCreate()
        if (Build.VERSION.SDK_INT >= 35) appScope.launch { publishWidgetPreviews(this@YoApplication) }
        if (com.google.firebase.FirebaseOptions.fromResource(this) != null) FirebaseApp.initializeApp(this)
    }

    private val appScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    val repository by lazy {
        Repository(this, appScope).also { it.start() }
    }
}

internal val Context.repository: Repository
    get() = (applicationContext as YoApplication).repository

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

class YoWork(context: Context, parameters: WorkerParameters) : CoroutineWorker(context, parameters) {
    override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
        val repo = applicationContext.repository
        val state = repo.state.first { !it.loading }
        if (state.broken) {
            if (inputData.getBoolean("urgent", false)) repo.pushFallback("stateBroken")
            return@withContext Result.failure()
        }
        if (state.username == null) return@withContext Result.success()
        try {
            when (inputData.getString("action")) {
                "token" -> {
                    if (FirebaseApp.getApps(applicationContext).isNotEmpty()) {
                        val token = Tasks.await(FirebaseMessaging.getInstance().token, 20, TimeUnit.SECONDS)
                        repo.registerPush(token)
                    }
                }
                "inbox" -> syncWithRetries(repo)
                "send" -> repo.sendWidgetYo(inputData.getString("friend"), inputData.getString("account"), inputData.getBoolean("latest", false))
                else -> return@withContext Result.failure()
            }
            Result.success()
        } catch (e: CancellationException) {
            throw e
        } catch (_: YoException.Replaced) {
            repo.signOut(replaced = true)
            Result.failure()
        } catch (e: Exception) {
            if (runAttemptCount == 0 && inputData.getBoolean("urgent", false)) repo.pushFallback(fallbackCode(e))
            if (runAttemptCount < 4) Result.retry() else Result.failure()
        }
    }

    /** As Signal's fetch after a push: the inbox pass is tried again while the phone's network wakes up (out of Doze,
     *  back on Wi-Fi: its first requests fail at once), for up to a minute, before the fallback says a Yo may have
     *  come. Two tries 3 s apart showed it for a network that took longer, and the Yo came with WorkManager's retry. */
    private suspend fun syncWithRetries(repo: Repository) {
        val deadline = SystemClock.elapsedRealtime() + 60_000
        for (pause in INBOX_PAUSES) {
            try { return repo.syncPush() } catch (e: Exception) {
                if (e is CancellationException || e is YoException.Replaced) throw e
                if (SystemClock.elapsedRealtime() + pause > deadline) throw e
            }
            kotlinx.coroutines.delay(pause)
        }
        repo.syncPush()
    }

    companion object {
        private val INBOX_PAUSES = listOf(2_000L, 4_000L, 8_000L, 15_000L, 25_000L)

        /** The counted fallback's code: what the last inbox pass failed with (iOS's codes), never a message. */
        fun fallbackCode(error: Throwable): String = when (error) {
            is YoException.Network -> "network"
            is YoException.Refused -> "hubStatus/${error.status}"
            is YoException.Protocol -> "protocol"
            is YoException.State -> "state"
            is YoException.Save -> "save"
            is YoException -> error::class.simpleName.orEmpty().replaceFirstChar { it.lowercase() }
            else -> "other/${error.javaClass.simpleName}"
        }

        fun enqueue(context: Context, action: String, friend: String? = null, account: String? = null,
                    urgent: Boolean = false, expedited: Boolean = false, latest: Boolean = false) {
            val builder = OneTimeWorkRequestBuilder<YoWork>()
                .setInputData(workDataOf("action" to action, "friend" to friend, "account" to account, "urgent" to urgent, "latest" to latest))
            if (expedited && Build.VERSION.SDK_INT >= 31) builder.setExpedited(androidx.work.OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
            val manager = WorkManager.getInstance(context)
            when (action) {
                "send" -> manager.enqueue(builder.build())
                // The newest token request replaces one still waiting: appended, it waited behind a retry that
                // WorkManager timed by the wall clock, and after the clock was set back (hours ahead before) the hub
                // got no token, so no push, for those hours. Asking for the token again is harmless.
                "token" -> manager.enqueueUniqueWork("yo-token", ExistingWorkPolicy.REPLACE, builder.build())
                // The same for a push's inbox pass: one waiting (a retry WorkManager times by the wall clock, and
                // after the clock was set back and a restart, for hours: no Yo was notified meanwhile) is replaced
                // and the new one runs now; one running is followed, not cut (what it took in would go unnotified).
                "inbox" -> manager.enqueueUniqueWork("yo-inbox", inboxPolicy(runCatching {
                    manager.getWorkInfosForUniqueWork("yo-inbox").get(2, TimeUnit.SECONDS).map { it.state }
                }.getOrNull()), builder.build())
                else -> manager.enqueueUniqueWork("yo-$action", ExistingWorkPolicy.APPEND_OR_REPLACE, builder.build())
            }
        }

        /** How a push's inbox pass joins the ones there ([states], null if unknown): after one running, else instead. */
        internal fun inboxPolicy(states: List<WorkInfo.State>?): ExistingWorkPolicy =
            if (states == null || WorkInfo.State.RUNNING in states) ExistingWorkPolicy.APPEND_OR_REPLACE
            else ExistingWorkPolicy.REPLACE

        fun registerIfConfigured(context: Context) {
            if (FirebaseApp.getApps(context).isEmpty()) return
            FirebaseMessaging.getInstance().isAutoInitEnabled = true
            enqueue(context, "token")
        }
    }
}
