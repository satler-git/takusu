package expo.modules.takusuagentservice

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.UserManager
import android.util.Log

class TakusuBootReceiver : BroadcastReceiver() {
    override fun onReceive(
        context: Context,
        intent: Intent?,
    ) {
        if (
            intent?.action != Intent.ACTION_BOOT_COMPLETED &&
            intent?.action != Intent.ACTION_LOCKED_BOOT_COMPLETED
        ) {
            return
        }

        if (!isUserUnlocked(context)) {
            return
        }

        try {
            restoreEvaluatorAlarm(context)
        } catch (e: Exception) {
            Log.w(TAG, "failed to restore evaluator alarm", e)
        }

        try {
            if (isUserUnlocked(context) && AmbientPreferences(context).isAmbientEnabled()) {
                AmbientNotificationHelper.postReArmNotification(context)
            }
        } catch (e: Exception) {
            Log.w(TAG, "failed to post re-arm notification", e)
        }
    }

    private fun restoreEvaluatorAlarm(context: Context) {
        val prefs = AmbientPreferences(context)
        val workersUrl = prefs.getWorkersUrl()
        val rootToken = prefs.getRootToken()
        val deviceId = prefs.getDeviceId()
        val localUrl = prefs.getLocalUrl()

        if (workersUrl.isBlank() || rootToken.isBlank()) {
            Log.i(TAG, "no evaluator credentials stored, skipping alarm restore")
            return
        }

        val alarmManager =
            context.getSystemService(Context.ALARM_SERVICE) as? AlarmManager
                ?: return

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

        val pending =
            PendingIntent.getBroadcast(
                context,
                REQUEST_CODE,
                intent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )

        val triggerAtMillis = System.currentTimeMillis() + BOOT_EVALUATOR_DELAY_MS

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
    }

    private fun isUserUnlocked(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) {
            return true
        }
        val userManager = context.getSystemService(Context.USER_SERVICE) as? UserManager
        return userManager?.isUserUnlocked ?: true
    }

    companion object {
        private const val ACTION_EVALUATE = "dev.satler.takusu.EVALUATE_EVENTS"
        private const val EXTRA_WORKERS_URL = "workers_url"
        private const val EXTRA_ROOT_TOKEN = "root_token"
        private const val EXTRA_DEVICE_ID = "device_id"
        private const val EXTRA_LOCAL_URL = "local_url"
        private const val REQUEST_CODE = 7381
        private const val BOOT_EVALUATOR_DELAY_MS = 60_000L
        private const val TAG = "TakusuBootReceiver"
    }
}
