package expo.modules.takusuagentservice

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.core.content.ContextCompat

class TakusuReArmReceiver : BroadcastReceiver() {
    override fun onReceive(
        context: Context,
        intent: Intent?,
    ) {
        if (intent?.action != ACTION_REARM) {
            return
        }

        if (!AmbientPreferences(context).isAmbientEnabled()) {
            return
        }

        try {
            // Android 12+ limits starting foreground services from the
            // background. This is invoked from a notification tap or an exact
            // alarm, both of which are on the allow-list, but failures are
            // still possible on heavily restricted devices.
            // TODO: fall back to a WorkManager expedited worker or a
            // dataSync/remoteMessaging foreground service on devices where
            // this path is disallowed.
            ContextCompat.startForegroundService(context, TakusuAgentService.startIntent(context))
        } catch (e: Exception) {
            Log.w(TAG, "failed to start ambient service from re-arm action", e)
        }
    }

    companion object {
        const val ACTION_REARM = "dev.satler.takusu.REARM_AMBIENT"
        private const val TAG = "TakusuReArmReceiver"
    }
}
