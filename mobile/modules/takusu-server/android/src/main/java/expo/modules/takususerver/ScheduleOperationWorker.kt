package expo.modules.takususerver

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
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
import java.net.ConnectException
import java.net.HttpURLConnection
import java.net.URL
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * Background worker that runs a core schedule operation (generate or reschedule)
 * and posts a progress/completion notification.
 *
 * The local Rust server may already be running if the app is in the foreground.
 * If not, the worker starts a temporary server, runs the operation, then stops it.
 */
class ScheduleOperationWorker(
    appContext: Context,
    workerParams: WorkerParameters,
) : CoroutineWorker(appContext, workerParams) {
    private val operation = inputData.getString(KEY_OPERATION) ?: ""
    private val operationId = inputData.getString(KEY_OPERATION_ID) ?: ""
    private val paramsJson = inputData.getString(KEY_PARAMS_JSON) ?: "{}"
    private val workersUrl = inputData.getString(KEY_WORKERS_URL) ?: ""
    private val token = inputData.getString(KEY_TOKEN) ?: ""
    private val port = inputData.getInt(KEY_PORT, DEFAULT_PORT)
    private val base = "http://127.0.0.1:$port"

    override suspend fun doWork(): Result {
        if (operation.isEmpty() || operationId.isEmpty() || workersUrl.isEmpty() || token.isEmpty()) {
            return Result.failure()
        }

        createChannel()
        val titles = operationTitles(operation)
        setForeground(createForegroundInfo(titles.inProgress, "開始中", 0, true))
        writeStatus("running")

        val path =
            when (operation) {
                "generate" -> "/api/schedule/generate"
                "reschedule" -> "/api/schedule/reschedule"
                else -> return Result.failure()
            }

        return try {
            val (code, response) = postWithServer(path)
            if (code in 200..299) {
                writeStatus("succeeded")
                setForeground(
                    createForegroundInfo(titles.success, "タップして確認", 100, false),
                )
                Result.success()
            } else {
                writeStatus("failed", response)
                setForeground(
                    createForegroundInfo(titles.error, response, 0, false),
                )
                Result.failure()
            }
        } catch (e: Exception) {
            // Let WorkManager handle coroutine cancellation as a cancellation,
            // not as a failure.
            if (e is CancellationException) throw e
            writeStatus("failed", e.message)
            setForeground(
                createForegroundInfo(titles.error, e.message ?: "unknown error", 0, false),
            )
            Result.failure()
        }
    }

    private suspend fun postWithServer(path: String): Pair<Int, String> =
        withContext(Dispatchers.IO) {
            var server: uniffi.takusu_android.TakusuServer? = null
            try {
                try {
                    postJson(path)
                } catch (e: ConnectException) {
                    try {
                        server = uniffi.takusu_android.TakusuServer()
                        server.start(port.toUShort(), workersUrl, token)
                    } catch (startError: Exception) {
                        throw Exception("一時サーバーの起動に失敗しました: ${startError.message}")
                    }
                    delay(500)
                    postJson(path)
                }
            } finally {
                server?.let {
                    try {
                        it.stop()
                    } catch (_: Exception) {
                    }
                    try {
                        it.destroy()
                    } catch (_: Exception) {
                    }
                    // Give the runtime a moment to fully drop its TcpListener
                    // and release the port before the foreground module tries
                    // to bind it again after a notification tap.
                    delay(100)
                }
            }
        }

    private fun postJson(path: String): Pair<Int, String> {
        val conn =
            (URL("$base$path").openConnection() as HttpURLConnection).apply {
                requestMethod = "POST"
                connectTimeout = CONNECT_TIMEOUT_MS
                readTimeout = READ_TIMEOUT_MS
                doOutput = true
                setRequestProperty("Authorization", "Bearer $token")
                setRequestProperty("Content-Type", "application/json")
            }
        return try {
            conn.outputStream.use { it.write(paramsJson.toByteArray(Charsets.UTF_8)) }
            val code = conn.responseCode
            val stream = if (code >= 400) conn.errorStream else conn.inputStream
            val body = stream?.bufferedReader()?.use { it.readText() } ?: ""
            Pair(code, body)
        } finally {
            conn.disconnect()
        }
    }

    private fun createChannel() {
        val manager =
            applicationContext.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, CHANNEL_NAME, NotificationManager.IMPORTANCE_LOW),
        )
    }

    private fun contentIntent(): PendingIntent {
        val intent =
            applicationContext.packageManager.getLaunchIntentForPackage(applicationContext.packageName)
                ?: Intent()
        intent.flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
        return PendingIntent.getActivity(
            applicationContext,
            0,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    private val notificationIconResId: Int by lazy {
        val appIcon =
            applicationContext.resources.getIdentifier(
                "notification_icon",
                "drawable",
                applicationContext.packageName,
            )
        if (appIcon != 0) appIcon else android.R.drawable.stat_notify_sync_noanim
    }

    private fun createForegroundInfo(
        title: String,
        text: String,
        progress: Int,
        ongoing: Boolean,
    ): ForegroundInfo {
        val notification: Notification =
            NotificationCompat
                .Builder(applicationContext, CHANNEL_ID)
                .setSmallIcon(notificationIconResId)
                .setColor(TakusuNotificationColor.color(applicationContext))
                .setContentTitle(title)
                .setContentText(text)
                .setOngoing(ongoing)
                .setProgress(100, progress, progress == 0 && ongoing)
                .setContentIntent(contentIntent())
                .setAutoCancel(!ongoing)
                .build()
        return ForegroundInfo(
            NOTIFICATION_ID,
            notification,
            ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC,
        )
    }

    private fun writeStatus(
        status: String,
        message: String? = null,
    ) {
        try {
            val file = statusFile(applicationContext)
            file.parentFile?.mkdirs()
            val json = JSONObject()
            json.put("id", operationId)
            json.put("operation", operation)
            json.put("status", status)
            json.put("message", message ?: "")
            file.writeText(json.toString())
        } catch (_: Exception) {
        }
    }

    data class OperationTitles(
        val inProgress: String,
        val success: String,
        val error: String,
    )

    private fun operationTitles(operation: String): OperationTitles =
        when (operation) {
            "generate" -> {
                OperationTitles(
                    inProgress = "タスクをスケジュール中",
                    success = "スケジュールが作成されました",
                    error = "スケジュールの作成に失敗しました",
                )
            }

            "reschedule" -> {
                OperationTitles(
                    inProgress = "タスクを再スケジュール中",
                    success = "再スケジュールが完了しました",
                    error = "再スケジュールに失敗しました",
                )
            }

            else -> {
                OperationTitles(
                    inProgress = operation,
                    success = "$operation が完了しました",
                    error = "$operation に失敗しました",
                )
            }
        }

    companion object {
        private const val DEFAULT_PORT = 3838
        private const val CONNECT_TIMEOUT_MS = 3_000
        private const val READ_TIMEOUT_MS = 9 * 60 * 1000 // 9 minutes; core scheduling can be slow
        private const val CHANNEL_ID = "background-schedule"
        private const val CHANNEL_NAME = "バックグラウンドスケジュール"
        private const val NOTIFICATION_ID = 0x5c48_0f00 // 'schedule'
        private const val WORK_NAME = "takusu-schedule-operation"

        private const val KEY_OPERATION = "operation"
        private const val KEY_OPERATION_ID = "operationId"
        private const val KEY_PARAMS_JSON = "paramsJson"
        private const val KEY_WORKERS_URL = "workersUrl"
        private const val KEY_TOKEN = "token"
        private const val KEY_PORT = "port"

        fun statusFile(context: Context): File = File(context.filesDir, "takusu/schedule-operation-status.json")

        fun clearStatus(context: Context) {
            try {
                statusFile(context).delete()
            } catch (_: Exception) {
            }
        }

        fun enqueue(
            context: Context,
            operation: String,
            operationId: String,
            paramsJson: String,
            workersUrl: String,
            token: String,
            port: Int,
        ) {
            val data =
                Data
                    .Builder()
                    .putString(KEY_OPERATION, operation)
                    .putString(KEY_OPERATION_ID, operationId)
                    .putString(KEY_PARAMS_JSON, paramsJson)
                    .putString(KEY_WORKERS_URL, workersUrl)
                    .putString(KEY_TOKEN, token)
                    .putInt(KEY_PORT, port)
                    .build()
            val request =
                OneTimeWorkRequestBuilder<ScheduleOperationWorker>()
                    .setInputData(data)
                    .setConstraints(
                        Constraints
                            .Builder()
                            .setRequiredNetworkType(NetworkType.CONNECTED)
                            .build(),
                    ).build()
            // Use REPLACE so only one schedule operation runs at a time.
            // A new request cancels any running one; the UI matches results
            // by operationId, so a stale completion does not update the list.
            WorkManager.getInstance(context).enqueueUniqueWork(
                WORK_NAME,
                ExistingWorkPolicy.REPLACE,
                request,
            )
        }
    }
}
