package expo.modules.takusualarms

import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent

class TakusuEvaluatorAlarmReceiver : BroadcastReceiver() {
    override fun onReceive(
        context: Context,
        intent: Intent?,
    ) {
        if (intent?.action != ACTION_EVALUATE) return
        val evaluatorIntent =
            Intent().apply {
                component =
                    ComponentName(
                        context,
                        "expo.modules.takususerver.TakusuEvaluatorAlarmReceiver",
                    )
                action = ACTION_EVALUATE
                putExtras(intent)
            }
        context.sendBroadcast(evaluatorIntent)
    }

    companion object {
        const val ACTION_EVALUATE = "dev.satler.takusu.EVALUATE_EVENTS"
        const val EXTRA_WORKERS_URL = "workers_url"
        const val EXTRA_ROOT_TOKEN = "root_token"
        const val EXTRA_DEVICE_ID = "device_id"
        const val DEFAULT_DEVICE_ID = "mobile"
        const val REQUEST_CODE = 7381
    }
}
