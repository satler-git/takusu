package expo.modules.takususerver

import android.app.AlarmManager
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.util.Log
import java.io.IOException
import java.util.concurrent.Executors
import uniffi.takusu_android.TakusuException
import uniffi.takusu_android.evaluateAndCommitEvents

class TakusuEvaluatorAlarmReceiver : BroadcastReceiver() {
    override fun onReceive(
        context: Context,
        intent: Intent?,
    ) {
        if (intent?.action != ACTION_EVALUATE) return
        val pendingResult = goAsync()
        val workersUrl = intent.getStringExtra(EXTRA_WORKERS_URL).orEmpty()
        val rootToken = intent.getStringExtra(EXTRA_ROOT_TOKEN).orEmpty()
        val deviceId = intent.getStringExtra(EXTRA_DEVICE_ID) ?: DEFAULT_DEVICE_ID
        val localUrl = intent.getStringExtra(EXTRA_LOCAL_URL).orEmpty()
        EXECUTOR.execute {
            try {
                val result = runEvaluator(localUrl, workersUrl, rootToken, deviceId)
                if (result != null) {
                    postResult(context, result.dueEventIds.size)
                    result.nextEvalAtMillis?.let { nextEvalAtMillis ->
                        reserveNextAlarm(context, intent, nextEvalAtMillis)
                    }
                }
            } finally {
                pendingResult.finish()
            }
        }
    }

    private fun runEvaluator(
        localUrl: String,
        workersUrl: String,
        rootToken: String,
        deviceId: String,
    ): EventEvaluationResult? {
        // Prefer the local `takusu-local` URL when one is configured. If the
        // local server is not reachable, fall back to workers so the exact
        // alarm can still commit events when the app is in the background.
        if (localUrl.isNotEmpty() && localUrl != workersUrl) {
            try {
                return evaluateAndCommitEvents(localUrl, rootToken, deviceId)
            } catch (e: Exception) {
                if (isNetworkOrServerError(e)) {
                    Log.w(TAG, "local server unavailable, falling back to workers: ${e.message}", e)
                } else {
                    throw e
                }
            }
        }
        return try {
            evaluateAndCommitEvents(workersUrl, rootToken, deviceId)
        } catch (e: Exception) {
            if (isNetworkOrServerError(e)) {
                Log.e(TAG, "workers event evaluation failed: ${e.message}", e)
                null
            } else {
                Log.e(TAG, "unexpected error during event evaluation: ${e.message}", e)
                throw e
            }
        }
    }

    private fun isNetworkOrServerError(error: Throwable): Boolean {
        if (error is TakusuException.Server) return true
        if (error is IOException) return true
        val packageName = error.javaClass.`package`?.name
        return packageName != null && packageName.startsWith("java.net.")
    }

    private fun postResult(
        context: Context,
        dueEventCount: Int,
    ) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        val channelId = "task-reminders"
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
            manager.createNotificationChannel(
                NotificationChannel(
                    channelId,
                    "Task reminders",
                    NotificationManager.IMPORTANCE_DEFAULT,
                ),
            )
        }
        val launchIntent = context.packageManager.getLaunchIntentForPackage(context.packageName)
        val contentIntent =
            launchIntent?.let {
                PendingIntent.getActivity(
                    context,
                    REQUEST_CODE,
                    it,
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
                )
            }
        val builder =
            if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.O) {
                Notification.Builder(context, channelId)
            } else {
                Notification.Builder(context)
            }
        val body =
            if (dueEventCount == 0) {
                "予定を確認できます"
            } else {
                "未確認のplanner eventが${dueEventCount}件あります"
            }
        val icon =
            context.resources.getIdentifier(
                "notification_icon",
                "drawable",
                context.packageName,
            )
        builder
            .setSmallIcon(icon)
            .setContentTitle("takusu")
            .setContentText(body)
            .setAutoCancel(true)
            .setContentIntent(contentIntent)
            .setPriority(Notification.PRIORITY_DEFAULT)
        manager.notify(NOTIFICATION_ID, builder.build())
    }

    private fun reserveNextAlarm(
        context: Context,
        source: Intent,
        triggerAtMillis: Long,
    ) {
        val alarmManager = context.getSystemService(AlarmManager::class.java) ?: return
        val nextIntent =
            Intent().apply {
                component =
                    ComponentName(
                        context,
                        TakusuEvaluatorAlarmReceiver::class.java,
                    )
                action = ACTION_EVALUATE
                putExtras(source)
            }
        val pendingIntent =
            PendingIntent.getBroadcast(
                context,
                REQUEST_CODE,
                nextIntent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.S &&
            alarmManager.canScheduleExactAlarms()
        ) {
            alarmManager.setExactAndAllowWhileIdle(
                AlarmManager.RTC_WAKEUP,
                triggerAtMillis,
                pendingIntent,
            )
        } else {
            alarmManager.setAndAllowWhileIdle(
                AlarmManager.RTC_WAKEUP,
                triggerAtMillis,
                pendingIntent,
            )
        }
    }

    companion object {
        const val ACTION_EVALUATE = "dev.satler.takusu.EVALUATE_EVENTS"
        const val EXTRA_WORKERS_URL = "workers_url"
        const val EXTRA_ROOT_TOKEN = "root_token"
        const val EXTRA_DEVICE_ID = "device_id"
        const val EXTRA_LOCAL_URL = "local_url"
        const val DEFAULT_DEVICE_ID = "mobile"
        const val REQUEST_CODE = 7381
        private const val NOTIFICATION_ID = 7381
        private val EXECUTOR = Executors.newSingleThreadExecutor()
        private const val TAG = "TakusuEvaluatorAlarmReceiver"
    }
}
