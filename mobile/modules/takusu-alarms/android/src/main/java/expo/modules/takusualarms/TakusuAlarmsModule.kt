package expo.modules.takusualarms

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import expo.modules.kotlin.exception.CodedException
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

class TakusuAlarmsModule : Module() {
    override fun definition() =
        ModuleDefinition {
            Name("TakusuAlarms")

            AsyncFunction("canScheduleExactAlarms") {
                val context =
                    appContext.reactContext
                        ?: throw CodedException(
                            "ERR_NO_CONTEXT",
                            "Android context is unavailable",
                            null,
                        )
                canScheduleExactAlarms(context)
            }

            AsyncFunction("requestExactAlarmPermission") {
                val context =
                    appContext.reactContext
                        ?: throw CodedException(
                            "ERR_NO_CONTEXT",
                            "Android context is unavailable",
                            null,
                        )
                val packageName = context.packageName
                val intent =
                    Intent(Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM).apply {
                        data = Uri.parse("package:$packageName")
                        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    }

                val activity = appContext.currentActivity
                if (activity != null) {
                    activity.startActivity(intent)
                } else {
                    context.startActivity(intent)
                }
                true
            }

            AsyncFunction("scheduleEvaluatorAlarm") {
                triggerAtMillis: Double,
                workersUrl: String,
                rootToken: String,
                deviceId: String,
                localUrl: String,
                ->
                val context =
                    appContext.reactContext
                        ?: throw CodedException(
                            "ERR_NO_CONTEXT",
                            "Android context is unavailable",
                            null,
                        )
                scheduleEvaluatorAlarm(
                    context,
                    triggerAtMillis.toLong(),
                    workersUrl,
                    rootToken,
                    deviceId,
                    localUrl,
                )
            }

            AsyncFunction("cancelEvaluatorAlarm") {
                val context =
                    appContext.reactContext
                        ?: throw CodedException(
                            "ERR_NO_CONTEXT",
                            "Android context is unavailable",
                            null,
                        )
                cancelEvaluatorAlarm(context)
            }
        }

    private fun canScheduleExactAlarms(context: Context): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val alarmManager = context.getSystemService(Context.ALARM_SERVICE) as? AlarmManager
            alarmManager?.canScheduleExactAlarms() ?: false
        } else {
            true
        }

    private fun pendingIntent(
        context: Context,
        workersUrl: String = "",
        rootToken: String = "",
        deviceId: String = DEFAULT_DEVICE_ID,
        localUrl: String = "",
    ): PendingIntent {
        val intent =
            Intent().apply {
                component =
                    ComponentName(
                        context.packageName,
                        "expo.modules.takususerver.TakusuEvaluatorAlarmReceiver",
                    )
                action = ACTION_EVALUATE
                putExtra(EXTRA_WORKERS_URL, workersUrl)
                putExtra(EXTRA_ROOT_TOKEN, rootToken)
                putExtra(EXTRA_DEVICE_ID, deviceId)
                putExtra(EXTRA_LOCAL_URL, localUrl)
            }
        return PendingIntent.getBroadcast(
            context,
            REQUEST_CODE,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    private fun scheduleEvaluatorAlarm(
        context: Context,
        triggerAtMillis: Long,
        workersUrl: String,
        rootToken: String,
        deviceId: String,
        localUrl: String,
    ): Boolean {
        val alarmManager =
            context.getSystemService(Context.ALARM_SERVICE) as? AlarmManager
                ?: return false
        val pending = pendingIntent(context, workersUrl, rootToken, deviceId, localUrl)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && alarmManager.canScheduleExactAlarms()) {
            alarmManager.setExactAndAllowWhileIdle(
                AlarmManager.RTC_WAKEUP,
                triggerAtMillis,
                pending,
            )
        } else {
            alarmManager.setAndAllowWhileIdle(
                AlarmManager.RTC_WAKEUP,
                triggerAtMillis,
                pending,
            )
        }
        return true
    }

    private fun cancelEvaluatorAlarm(context: Context): Boolean {
        val alarmManager =
            context.getSystemService(Context.ALARM_SERVICE) as? AlarmManager
                ?: return false
        alarmManager.cancel(pendingIntent(context))
        return true
    }

    companion object {
        const val ACTION_EVALUATE = "dev.satler.takusu.EVALUATE_EVENTS"
        const val EXTRA_WORKERS_URL = "workers_url"
        const val EXTRA_ROOT_TOKEN = "root_token"
        const val EXTRA_DEVICE_ID = "device_id"
        const val EXTRA_LOCAL_URL = "local_url"
        const val DEFAULT_DEVICE_ID = "mobile"
        const val REQUEST_CODE = 7381
    }
}
