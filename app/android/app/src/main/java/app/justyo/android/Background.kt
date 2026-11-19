package app.justyo.android

import android.app.Application
import android.content.Context
import android.os.Build
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraManager
import androidx.camera.camera2.Camera2Config
import androidx.camera.core.CameraSelector
import androidx.camera.core.CameraXConfig
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequestBuilder
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
        if (com.google.firebase.FirebaseOptions.fromResource(this) != null) FirebaseApp.initializeApp(this)
    }

    val repository by lazy {
        Repository(this, CoroutineScope(SupervisorJob() + Dispatchers.IO)).also { it.start() }
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
            if (inputData.getBoolean("urgent", false)) repo.pushFallback()
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
                "inbox" -> repo.syncPush()
                "send" -> repo.sendWidgetYo(inputData.getString("friend"), inputData.getString("account"), inputData.getBoolean("latest", false))
                else -> return@withContext Result.failure()
            }
            Result.success()
        } catch (e: CancellationException) {
            throw e
        } catch (_: YoException.Replaced) {
            repo.signOut(replaced = true)
            Result.failure()
        } catch (_: Exception) {
            if (runAttemptCount == 0 && inputData.getBoolean("urgent", false)) repo.pushFallback()
            if (runAttemptCount < 4) Result.retry() else Result.failure()
        }
    }

    companion object {
        fun enqueue(context: Context, action: String, friend: String? = null, account: String? = null,
                    urgent: Boolean = false, expedited: Boolean = false, latest: Boolean = false) {
            val builder = OneTimeWorkRequestBuilder<YoWork>()
                .setInputData(workDataOf("action" to action, "friend" to friend, "account" to account, "urgent" to urgent, "latest" to latest))
            if (expedited && Build.VERSION.SDK_INT >= 31) builder.setExpedited(androidx.work.OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
            val manager = WorkManager.getInstance(context)
            if (action == "send") manager.enqueue(builder.build())
            else manager.enqueueUniqueWork("yo-$action", ExistingWorkPolicy.APPEND_OR_REPLACE, builder.build())
        }

        fun registerIfConfigured(context: Context) {
            if (FirebaseApp.getApps(context).isEmpty()) return
            FirebaseMessaging.getInstance().isAutoInitEnabled = true
            enqueue(context, "token")
        }
    }
}
