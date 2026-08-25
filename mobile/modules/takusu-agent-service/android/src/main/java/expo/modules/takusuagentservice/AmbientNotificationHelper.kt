package expo.modules.takusuagentservice

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.graphics.Color
import android.os.Build
import androidx.core.app.NotificationCompat

private const val CHANNEL_ID_AGENT = "takusu-agent"
private const val CHANNEL_NAME_AGENT = "takusu 常時聴取"
private const val CHANNEL_ID_REARM = "takusu-rearm"
private const val CHANNEL_NAME_REARM = "常時聴取を再開"
private const val CHANNEL_ID_RESULT = "takusu-result"
private const val CHANNEL_NAME_RESULT = "takusu 結果"

private const val NOTIFICATION_ID_AGENT = 0x7382_0001
private const val NOTIFICATION_ID_RESULT = 0x7382_0002
private const val NOTIFICATION_ID_REARM = 0x7382_0003

private const val REQUEST_CODE_STOP = 0x7382_0101
private const val REQUEST_CODE_REARM = 0x7382_0102
private const val REQUEST_CODE_OPEN = 0x7382_0103

object AmbientNotificationHelper {
    fun createChannels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return
        }

        val manager =
            context.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager
                ?: return

        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_ID_AGENT,
                CHANNEL_NAME_AGENT,
                NotificationManager.IMPORTANCE_LOW,
            ),
        )

        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_ID_REARM,
                CHANNEL_NAME_REARM,
                NotificationManager.IMPORTANCE_DEFAULT,
            ),
        )

        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_ID_RESULT,
                CHANNEL_NAME_RESULT,
                NotificationManager.IMPORTANCE_HIGH,
            ),
        )
    }

    fun createPersistentNotification(
        context: Context,
        stateText: String,
    ): Notification {
        val stopPending = stopServicePendingIntent(context)

        return NotificationCompat
            .Builder(context, CHANNEL_ID_AGENT)
            .setSmallIcon(getSmallIcon(context))
            .setColor(notificationColor(context))
            .setContentTitle("takusu 常時聴取")
            .setContentText(stateText)
            .setOngoing(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .addAction(0, "停止", stopPending)
            .build()
    }

    fun postReArmNotification(context: Context) {
        createChannels(context)

        val manager =
            context.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager
                ?: return

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && !manager.areNotificationsEnabled()) {
            return
        }

        val reArmPending = reArmPendingIntent(context)

        val notification =
            NotificationCompat
                .Builder(context, CHANNEL_ID_REARM)
                .setSmallIcon(getSmallIcon(context))
                .setColor(notificationColor(context))
                .setContentTitle("takusu")
                .setContentText("常時聴取を再開")
                .setAutoCancel(true)
                .setContentIntent(reArmPending)
                .addAction(0, "再開", reArmPending)
                .build()

        manager.notify(NOTIFICATION_ID_REARM, notification)
    }

    fun postResultNotification(
        context: Context,
        text: String,
    ) {
        createChannels(context)

        val manager =
            context.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager
                ?: return

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N && !manager.areNotificationsEnabled()) {
            return
        }

        val openPending = openAppPendingIntent(context)

        val notification =
            NotificationCompat
                .Builder(context, CHANNEL_ID_RESULT)
                .setSmallIcon(getSmallIcon(context))
                .setColor(notificationColor(context))
                .setContentTitle("takusu 結果")
                .setContentText(text)
                .setStyle(NotificationCompat.BigTextStyle().bigText(text))
                .setAutoCancel(true)
                .setContentIntent(openPending)
                .addAction(0, "開く", openPending)
                .build()

        manager.notify(NOTIFICATION_ID_RESULT, notification)
    }

    private fun stopServicePendingIntent(context: Context): PendingIntent {
        val intent =
            Intent(context, TakusuAgentService::class.java).apply {
                action = TakusuAgentService.ACTION_STOP
            }

        return PendingIntent.getService(
            context,
            REQUEST_CODE_STOP,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    private fun reArmPendingIntent(context: Context): PendingIntent {
        val intent =
            Intent(context, TakusuReArmReceiver::class.java).apply {
                action = TakusuReArmReceiver.ACTION_REARM
            }

        return PendingIntent.getBroadcast(
            context,
            REQUEST_CODE_REARM,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    private fun openAppPendingIntent(context: Context): PendingIntent {
        val launchIntent =
            context.packageManager.getLaunchIntentForPackage(context.packageName)
                ?: Intent()

        launchIntent.flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP

        return PendingIntent.getActivity(
            context,
            REQUEST_CODE_OPEN,
            launchIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    private fun getSmallIcon(context: Context): Int {
        val res =
            context.resources.getIdentifier(
                "notification_icon",
                "drawable",
                context.packageName,
            )
        return if (res != 0) res else android.R.drawable.stat_notify_chat
    }

    private fun notificationColor(context: Context): Int {
        val prefs = context.getSharedPreferences("takusu_theme_prefs", Context.MODE_PRIVATE)
        val theme = prefs.getString("takusu_theme", "light") ?: "light"
        return when (theme) {
            "dark", "catppuccin" -> Color.parseColor("#9B8BC4")
            "aura-soft-dark" -> Color.parseColor("#A48BD6")
            else -> Color.parseColor("#7261A3")
        }
    }
}
