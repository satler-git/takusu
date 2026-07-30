package expo.modules.takususerver

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.pm.ServiceInfo
import androidx.core.app.NotificationCompat
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.Data
import androidx.work.ExistingWorkPolicy
import androidx.work.ForegroundInfo
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONObject
import uniffi.takusu_android.downloadModel

class ModelDownloadWorker(
    appContext: Context,
    workerParams: WorkerParameters,
) : CoroutineWorker(appContext, workerParams) {
    private val modelId = inputData.getString(KEY_MODEL_ID) ?: ""
    private val channelId = "agent-model-downloads"
    private val notificationId = modelId.hashCode().and(0x7fffffff).or(1)

    override suspend fun doWork(): Result {
        createChannel()
        val modelRoot = File(applicationContext.noBackupFilesDir, "takusu/models")
        val statusFile = File(applicationContext.filesDir, "takusu/model-status-$modelId.json")
        modelRoot.mkdirs()
        statusFile.parentFile?.mkdirs()
        setForeground(createForegroundInfo("音声モデルを準備中", "開始中", 0, false))

        return try {
            coroutineScope {
                val job =
                    async(Dispatchers.IO) {
                        downloadModel(modelRoot.absolutePath, modelId, statusFile.absolutePath)
                    }
                while (!job.isCompleted) {
                    updateProgress(statusFile)
                    delay(500)
                }
                job.await()
            }
            setForeground(createForegroundInfo("音声モデルの準備完了", modelId, 100, true))
            Result.success()
        } catch (error: Exception) {
            setForeground(createForegroundInfo("音声モデルの準備に失敗", error.message ?: "unknown error", 0, true))
            if (runAttemptCount + 1 >= MAX_ATTEMPTS) {
                Result.failure()
            } else {
                Result.retry()
            }
        }
    }

    private suspend fun updateProgress(statusFile: File) {
        val status =
            withContext(Dispatchers.IO) {
                if (statusFile.exists()) statusFile.readText() else null
            } ?: return
        val json =
            try {
                JSONObject(status)
            } catch (_: Exception) {
                return
            }
        val downloaded = json.optLong("downloadedBytes", 0)
        val total = json.optLong("totalBytes", 0)
        val stage = json.optString("stage", "準備中")
        val percent = if (total > 0) ((downloaded * 100) / total).toInt() else 0
        setForeground(createForegroundInfo("音声モデルを準備中", "$modelId: $stage", percent, false))
    }

    private fun createChannel() {
        val manager = applicationContext.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        manager.createNotificationChannel(
            NotificationChannel(channelId, "音声モデルの準備", NotificationManager.IMPORTANCE_LOW),
        )
    }

    private fun createForegroundInfo(
        title: String,
        text: String,
        progress: Int,
        done: Boolean,
    ): ForegroundInfo {
        val notification: Notification =
            NotificationCompat
                .Builder(applicationContext, channelId)
                .setSmallIcon(android.R.drawable.stat_sys_download)
                .setColor(TakusuNotificationColor.color(applicationContext))
                .setContentTitle(title)
                .setContentText(text)
                .setOngoing(!done)
                .setProgress(100, progress, progress == 0 && !done)
                .build()
        return ForegroundInfo(
            notificationId,
            notification,
            ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
        )
    }

    companion object {
        private const val KEY_MODEL_ID = "modelId"
        private const val WORK_PREFIX = "takusu-agent-model-"
        private const val MAX_ATTEMPTS = 3

        fun enqueue(
            context: Context,
            modelId: String,
        ) {
            val request =
                OneTimeWorkRequestBuilder<ModelDownloadWorker>()
                    .setInputData(Data.Builder().putString(KEY_MODEL_ID, modelId).build())
                    .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                    .build()
            WorkManager.getInstance(context).enqueueUniqueWork(
                WORK_PREFIX + modelId,
                ExistingWorkPolicy.REPLACE,
                request,
            )
        }
    }
}
