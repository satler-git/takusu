package expo.modules.takusualarms

import android.app.AlarmManager
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
        }

    private fun canScheduleExactAlarms(context: Context): Boolean =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val alarmManager = context.getSystemService(Context.ALARM_SERVICE) as? AlarmManager
            alarmManager?.canScheduleExactAlarms() ?: false
        } else {
            true
        }
}
