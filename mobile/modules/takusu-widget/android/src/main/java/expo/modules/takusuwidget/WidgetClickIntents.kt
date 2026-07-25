package expo.modules.takusuwidget

import android.app.PendingIntent
import android.content.Context
import android.content.Intent

/**
 * Creates the [PendingIntent]s used by the widget click handlers.
 *
 * These are factored out of [TakusuWidgetProvider] so the Android 14+
 * `PendingIntent` mutability / explicit-component requirements can be
 * verified by unit tests.
 */
internal object WidgetClickIntents {
    fun createListPendingIntent(
        context: Context,
        launchIntent: Intent,
    ): PendingIntent {
        val comp =
            requireNotNull(launchIntent.component) {
                "launchIntent must have an explicit component for the list PendingIntent"
            }
        val listTemplateIntent =
            Intent(Intent.ACTION_VIEW).apply {
                flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
                component = comp
                setPackage(comp.packageName)
            }
        return PendingIntent.getActivity(
            context,
            0,
            listTemplateIntent,
            PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
    }

    fun createRootPendingIntent(
        context: Context,
        launchIntent: Intent,
    ): PendingIntent =
        PendingIntent.getActivity(
            context,
            0,
            launchIntent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
}
