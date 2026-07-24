package expo.modules.takusuwidget

import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.util.Log
import android.widget.RemoteViews
import androidx.core.widget.RemoteViewsCompat
import org.json.JSONArray
import org.json.JSONObject

private const val TAG = "TakusuWidgetProvider"
private const val WIDE_DP = 250
private const val TALL_DP = 90

private enum class WidgetSize {
    W4x2,
    W4x1,
    W2x2,
    W2x1,
}

private data class Snapshot(
    val doing: UpcomingTask?,
    val upcoming: List<UpcomingTask>,
    val unscheduledCount: Int,
    val serverTz: String?,
    val scheme: String?,
)

/**
 * The AppWidgetProvider for the takusu home screen widget.
 *
 * It reads the latest snapshot from SharedPreferences (written by
 * [WidgetUpdateWorker] or [TakusuWidgetModule]) and renders it into
 * a RemoteViews tree. If no snapshot is available yet, it shows a placeholder.
 *
 * The provider picks a layout based on the widget's current size bucket:
 * 4x2, 4x1, 2x2, or 2x1.
 *
 * Upcoming-task lists are populated with [RemoteViewsCompat.RemoteCollectionItems]
 * instead of a [RemoteViewsService], so the list is updated directly as part
 * of the [RemoteViews] tree and no notifyAppWidgetViewDataChanged call is needed.
 */
class TakusuWidgetProvider : AppWidgetProvider() {
    override fun onUpdate(
        context: Context,
        appWidgetManager: AppWidgetManager,
        appWidgetIds: IntArray,
    ) {
        for (id in appWidgetIds) {
            try {
                updateWidget(context, appWidgetManager, id)
            } catch (e: Throwable) {
                Log.e(TAG, "Failed to update widget id=$id", e)
            }
        }
    }

    override fun onAppWidgetOptionsChanged(
        context: Context,
        appWidgetManager: AppWidgetManager,
        appWidgetId: Int,
        newOptions: Bundle,
    ) {
        try {
            updateWidget(context, appWidgetManager, appWidgetId, newOptions)
        } catch (e: Throwable) {
            Log.e(TAG, "Failed to update widget id=$appWidgetId on options changed", e)
        }
    }

    override fun onEnabled(context: Context) {
        super.onEnabled(context)
        try {
            WidgetUpdateWorker.schedule(context)
        } catch (e: Throwable) {
            Log.w(TAG, "Failed to schedule widget update worker", e)
        }
    }

    override fun onDisabled(context: Context) {
        super.onDisabled(context)
        try {
            WidgetUpdateWorker.cancel(context)
        } catch (e: Throwable) {
            Log.w(TAG, "Failed to cancel widget update worker", e)
        }
    }

    companion object {
        fun updateWidget(context: Context) {
            val manager = AppWidgetManager.getInstance(context)
            val ids = manager.getAppWidgetIds(ComponentName(context, TakusuWidgetProvider::class.java))
            for (id in ids) {
                updateWidget(context, manager, id)
            }
        }

        private fun updateWidget(
            context: Context,
            manager: AppWidgetManager,
            widgetId: Int,
        ) {
            updateWidget(context, manager, widgetId, manager.getAppWidgetOptions(widgetId))
        }

        private fun updateWidget(
            context: Context,
            manager: AppWidgetManager,
            widgetId: Int,
            options: Bundle,
        ) {
            val prefs = context.getSharedPreferences(WidgetUpdateWorker.PREFS_NAME, Context.MODE_PRIVATE)
            val snapshotJson = prefs.getString(WidgetUpdateWorker.KEY_SNAPSHOT, null)
            val updatedAt = prefs.getLong(WidgetUpdateWorker.KEY_UPDATED_AT, 0L)
            val snapshot =
                try {
                    snapshotJson?.let { parseSnapshot(it) }
                } catch (_: Exception) {
                    null
                }
            val zone = parseZone(snapshot?.serverTz)
            val scheme =
                (
                    snapshot?.scheme ?: prefs.getString(
                        WidgetUpdateWorker.KEY_SCHEME,
                        null,
                    )
                ).takeIf { !it.isNullOrEmpty() }
                    ?: "takusu"

            val size = resolveSize(options)
            val layout =
                when (size) {
                    WidgetSize.W4x2 -> R.layout.takusu_widget_4x2
                    WidgetSize.W4x1 -> R.layout.takusu_widget_4x1
                    WidgetSize.W2x2 -> R.layout.takusu_widget_2x2
                    WidgetSize.W2x1 -> R.layout.takusu_widget_2x1
                }
            val views = RemoteViews(context.packageName, layout)

            when (size) {
                WidgetSize.W4x2 -> render4x2(context, views, snapshot, updatedAt, widgetId, zone, scheme)
                WidgetSize.W4x1 -> render4x1(context, views, snapshot, updatedAt, zone, scheme)
                WidgetSize.W2x2 -> render2x2(context, views, snapshot, updatedAt, widgetId, zone, scheme)
                WidgetSize.W2x1 -> render2x1(context, views, snapshot, updatedAt, zone, scheme)
            }

            setClickIntents(context, views, size)
            manager.updateAppWidget(widgetId, views)
        }

        private fun resolveSize(options: Bundle): WidgetSize {
            val width = options.getInt(AppWidgetManager.OPTION_APPWIDGET_MIN_WIDTH, 0)
            val height = options.getInt(AppWidgetManager.OPTION_APPWIDGET_MIN_HEIGHT, 0)
            val wide = width >= WIDE_DP
            val tall = height >= TALL_DP
            return when {
                wide && tall -> WidgetSize.W4x2
                wide && !tall -> WidgetSize.W4x1
                !wide && tall -> WidgetSize.W2x2
                else -> WidgetSize.W2x1
            }
        }

        private fun render4x2(
            context: Context,
            views: RemoteViews,
            snapshot: Snapshot?,
            updatedAt: Long,
            widgetId: Int,
            zone: java.time.ZoneId,
            scheme: String,
        ) {
            views.setTextViewText(R.id.widget_updated, formatUpdated(updatedAt))

            val doing = snapshot?.doing
            if (doing != null) {
                views.setViewVisibility(R.id.widget_doing_section, android.view.View.VISIBLE)
                views.setTextViewText(R.id.widget_doing_title, doing.title)
                setOptionalTime(views, R.id.widget_doing_time, formatTimeRange(doing.startAt, doing.endAt, zone))
                val progress = computeProgress(doing.startAt, doing.endAt, zone)
                views.setProgressBar(R.id.widget_doing_progress, 100, progress, false)
                setOptionalTime(views, R.id.widget_doing_remaining, computeRemaining(doing.endAt, zone))
            } else {
                views.setViewVisibility(R.id.widget_doing_section, android.view.View.GONE)
            }

            val upcoming = snapshot?.upcoming ?: emptyList()
            val hasUpcoming = upcoming.isNotEmpty()
            val hasContent = doing != null || hasUpcoming

            views.setViewVisibility(
                R.id.widget_upcoming_label,
                if (hasUpcoming) android.view.View.VISIBLE else android.view.View.GONE,
            )
            views.setViewVisibility(
                R.id.widget_upcoming_list,
                if (hasUpcoming) android.view.View.VISIBLE else android.view.View.GONE,
            )
            views.setViewVisibility(
                R.id.widget_empty,
                if (hasContent) android.view.View.GONE else android.view.View.VISIBLE,
            )

            if (hasUpcoming) {
                val items =
                    buildCollectionItems(
                        context,
                        upcoming,
                        maxItems = 4,
                        overflowLayout = true,
                        zone = zone,
                        scheme = scheme,
                    )
                try {
                    RemoteViewsCompat.setRemoteAdapter(context, views, widgetId, R.id.widget_upcoming_list, items)
                } catch (e: Exception) {
                    Log.w(TAG, "Failed to set widget upcoming list adapter", e)
                    // Hide the list and label. Leave widget_empty hidden because showing
                    // "今日の予定はまだありません" would be inaccurate when there are upcoming tasks.
                    views.setViewVisibility(R.id.widget_upcoming_list, android.view.View.GONE)
                    views.setViewVisibility(R.id.widget_upcoming_label, android.view.View.GONE)
                }
            }

            val unscheduled = snapshot?.unscheduledCount ?: 0
            if (unscheduled > 0) {
                views.setViewVisibility(R.id.widget_unscheduled, android.view.View.VISIBLE)
                views.setTextViewText(R.id.widget_unscheduled, "未スケジュール $unscheduled")
            } else {
                views.setViewVisibility(R.id.widget_unscheduled, android.view.View.GONE)
            }
        }

        private fun render4x1(
            context: Context,
            views: RemoteViews,
            snapshot: Snapshot?,
            updatedAt: Long,
            zone: java.time.ZoneId,
            scheme: String,
        ) {
            val primary = primaryTask(snapshot)
            if (primary == null) {
                views.setViewVisibility(R.id.widget_label, android.view.View.GONE)
                views.setTextViewText(R.id.widget_title, "今日の予定はまだありません")
                views.setViewVisibility(R.id.widget_time, android.view.View.GONE)
                views.setViewVisibility(R.id.widget_dot, android.view.View.GONE)
                renderRemaining(views, snapshot)
                return
            }
            views.setViewVisibility(R.id.widget_label, android.view.View.VISIBLE)
            views.setTextViewText(R.id.widget_label, if (primary == snapshot?.doing) "進行中" else "次")
            views.setTextViewText(R.id.widget_title, primary.title)
            setOptionalTime(views, R.id.widget_time, formatTime(primary.startAt, zone))
            views.setViewVisibility(R.id.widget_dot, android.view.View.VISIBLE)
            views.setTextColor(R.id.widget_dot, WidgetDotColors.color(context, primary))
            renderRemaining(views, snapshot)
        }

        private fun render2x2(
            context: Context,
            views: RemoteViews,
            snapshot: Snapshot?,
            updatedAt: Long,
            widgetId: Int,
            zone: java.time.ZoneId,
            scheme: String,
        ) {
            views.setTextViewText(R.id.widget_updated, formatUpdated(updatedAt))

            val primary = primaryTask(snapshot)
            if (primary == null) {
                views.setViewVisibility(R.id.widget_mini_main, android.view.View.GONE)
                views.setViewVisibility(R.id.widget_empty, android.view.View.VISIBLE)
                views.setViewVisibility(R.id.widget_mini_list, android.view.View.GONE)
                renderRemaining(views, snapshot)
                return
            }

            views.setViewVisibility(R.id.widget_mini_main, android.view.View.VISIBLE)
            views.setViewVisibility(R.id.widget_empty, android.view.View.GONE)
            views.setTextViewText(R.id.widget_label, if (primary == snapshot?.doing) "進行中" else "次")
            views.setTextViewText(R.id.widget_title, primary.title)
            setOptionalTime(views, R.id.widget_meta, formatTimeRange(primary.startAt, primary.endAt, zone))

            val extras =
                if (primary == snapshot?.doing) {
                    snapshot?.upcoming?.take(2) ?: emptyList()
                } else {
                    snapshot?.upcoming?.drop(1)?.take(2) ?: emptyList()
                }
            if (extras.isNotEmpty()) {
                views.setViewVisibility(R.id.widget_mini_list, android.view.View.VISIBLE)
                val items =
                    buildCollectionItems(
                        context,
                        extras,
                        maxItems = 2,
                        overflowLayout = false,
                        mini = true,
                        zone = zone,
                        scheme = scheme,
                    )
                try {
                    RemoteViewsCompat.setRemoteAdapter(context, views, widgetId, R.id.widget_mini_list, items)
                } catch (e: Exception) {
                    Log.w(TAG, "Failed to set widget mini list adapter", e)
                    views.setViewVisibility(R.id.widget_mini_list, android.view.View.GONE)
                }
            } else {
                views.setViewVisibility(R.id.widget_mini_list, android.view.View.GONE)
            }
            renderRemaining(views, snapshot)
        }

        private fun render2x1(
            context: Context,
            views: RemoteViews,
            snapshot: Snapshot?,
            updatedAt: Long,
            zone: java.time.ZoneId,
            scheme: String,
        ) {
            val primary = primaryTask(snapshot)
            if (primary == null) {
                views.setTextViewText(R.id.widget_title, "予定なし")
                views.setViewVisibility(R.id.widget_time, android.view.View.GONE)
                views.setViewVisibility(R.id.widget_dot, android.view.View.GONE)
                renderRemaining(views, snapshot)
                return
            }

            views.setTextViewText(R.id.widget_title, primary.title)
            setOptionalTime(views, R.id.widget_time, formatTime(primary.startAt, zone))
            views.setViewVisibility(R.id.widget_dot, android.view.View.VISIBLE)
            views.setTextColor(R.id.widget_dot, WidgetDotColors.color(context, primary))
            renderRemaining(views, snapshot)
        }

        private fun renderRemaining(
            views: RemoteViews,
            snapshot: Snapshot?,
        ) {
            val n = todayRemaining(snapshot)
            if (n > 0) {
                views.setViewVisibility(R.id.widget_remaining, android.view.View.VISIBLE)
                views.setTextViewText(R.id.widget_remaining, "残 $n")
            } else {
                views.setViewVisibility(R.id.widget_remaining, android.view.View.GONE)
            }
        }

        private fun setOptionalTime(
            views: RemoteViews,
            viewId: Int,
            text: String,
        ) {
            if (text.isBlank()) {
                views.setViewVisibility(viewId, android.view.View.GONE)
            } else {
                views.setViewVisibility(viewId, android.view.View.VISIBLE)
                views.setTextViewText(viewId, text)
            }
        }

        private fun buildCollectionItems(
            context: Context,
            tasks: List<UpcomingTask>,
            maxItems: Int,
            overflowLayout: Boolean,
            mini: Boolean = false,
            zone: java.time.ZoneId,
            scheme: String,
        ): RemoteViewsCompat.RemoteCollectionItems {
            val builder =
                RemoteViewsCompat.RemoteCollectionItems
                    .Builder()
                    .setViewTypeCount(if (overflowLayout) 2 else 1)
                    .setHasStableIds(false)

            val shown = tasks.take(maxItems)
            for ((index, task) in shown.withIndex()) {
                builder.addItem(index.toLong(), buildItemRemoteViews(context, task, mini, zone, scheme))
            }
            val overflow = tasks.size - maxItems
            if (overflowLayout && overflow > 0) {
                builder.addItem(
                    shown.size.toLong(),
                    buildOverflowRemoteViews(context, overflow, scheme),
                )
            }
            return builder.build()
        }

        private fun buildItemRemoteViews(
            context: Context,
            task: UpcomingTask,
            mini: Boolean,
            zone: java.time.ZoneId,
            scheme: String,
        ): RemoteViews {
            val layout = if (mini) R.layout.takusu_widget_mini_item else R.layout.takusu_widget_item
            val views = RemoteViews(context.packageName, layout)
            setOptionalTime(views, R.id.widget_item_time, formatTime(task.startAt, zone))
            views.setTextColor(R.id.widget_item_dot, WidgetDotColors.color(context, task))
            views.setTextViewText(R.id.widget_item_title, task.title)

            if (!mini) {
                if (task.fixed) {
                    views.setViewVisibility(R.id.widget_item_fixed, android.view.View.VISIBLE)
                } else {
                    views.setViewVisibility(R.id.widget_item_fixed, android.view.View.GONE)
                }
            }

            val taskId = task.id
            val fillIn = Intent(Intent.ACTION_VIEW, Uri.parse("$scheme://task/$taskId"))
            views.setOnClickFillInIntent(R.id.widget_item_root, fillIn)
            return views
        }

        private fun buildOverflowRemoteViews(
            context: Context,
            overflow: Int,
            scheme: String,
        ): RemoteViews {
            val views = RemoteViews(context.packageName, R.layout.takusu_widget_item_overflow)
            views.setTextViewText(R.id.widget_item_overflow_text, "他 $overflow 件")
            val fillIn = Intent(Intent.ACTION_VIEW, Uri.parse("$scheme://tasks"))
            views.setOnClickFillInIntent(R.id.widget_item_root, fillIn)
            return views
        }

        private fun setClickIntents(
            context: Context,
            views: RemoteViews,
            size: WidgetSize,
        ) {
            val launchIntent = context.packageManager.getLaunchIntentForPackage(context.packageName)
            if (launchIntent?.component == null) return

            val listViewId =
                when (size) {
                    WidgetSize.W4x2 -> R.id.widget_upcoming_list
                    WidgetSize.W2x2 -> R.id.widget_mini_list
                    else -> null
                }
            if (listViewId != null) {
                views.setPendingIntentTemplate(
                    listViewId,
                    WidgetClickIntents.createListPendingIntent(context, launchIntent),
                )
            }

            val launchPendingIntent =
                WidgetClickIntents.createRootPendingIntent(context, launchIntent)
            views.setOnClickPendingIntent(R.id.widget_root, launchPendingIntent)
            if (size == WidgetSize.W4x2) {
                views.setOnClickPendingIntent(R.id.widget_open_btn, launchPendingIntent)
            }
        }

        private fun primaryTask(snapshot: Snapshot?): UpcomingTask? {
            if (snapshot == null) return null
            return snapshot.doing ?: snapshot.upcoming.firstOrNull()
        }

        private fun todayRemaining(snapshot: Snapshot?): Int {
            if (snapshot == null) return 0
            val doingCount = if (snapshot.doing != null) 1 else 0
            return doingCount + snapshot.upcoming.size + snapshot.unscheduledCount
        }

        private fun parseSnapshot(json: String): Snapshot {
            val obj = JSONObject(json)
            val doing = parseTask(obj.optJSONObject("doing"))
            val upcoming = mutableListOf<UpcomingTask>()
            val arr = obj.optJSONArray("upcoming") ?: JSONArray()
            for (i in 0 until arr.length()) {
                parseTask(arr.optJSONObject(i))?.let { upcoming.add(it) }
            }
            val fallbackDoing = parseLegacyDoing(obj.optJSONArray("doing_titles"))
            return Snapshot(
                doing = doing ?: fallbackDoing,
                upcoming = upcoming,
                unscheduledCount = obj.optInt("unscheduled_count", 0),
                serverTz = optStringOrNull(obj, "server_tz"),
                scheme = optStringOrNull(obj, "scheme"),
            )
        }

        private fun parseTask(obj: JSONObject?): UpcomingTask? {
            if (obj == null || obj == JSONObject.NULL) return null
            return try {
                UpcomingTask(
                    id = obj.getString("id"),
                    title = obj.getString("title"),
                    startAt = optStringOrNull(obj, "start_at"),
                    endAt = obj.getString("end_at"),
                    abandonability = obj.optDouble("abandonability", 0.75),
                    fixed = obj.optBoolean("fixed", false),
                )
            } catch (_: Exception) {
                null
            }
        }

        private fun parseLegacyDoing(titles: JSONArray?): UpcomingTask? {
            if (titles == null || titles.length() == 0) return null
            return UpcomingTask(
                id = "",
                title = titles.getString(0),
                startAt = null,
                endAt = "",
                abandonability = 0.75,
                fixed = false,
            )
        }

        private fun optStringOrNull(
            obj: JSONObject,
            key: String,
        ): String? = if (obj.isNull(key) || !obj.has(key)) null else obj.optString(key, "")

        private fun computeProgress(
            startAt: String?,
            endAt: String,
            zone: java.time.ZoneId,
        ): Int {
            val start = parseIso(startAt, zone) ?: return 0
            val end = parseIso(endAt, zone) ?: return 0
            val now = System.currentTimeMillis()
            if (now >= end) return 100
            if (now <= start) return 0
            return ((now - start) * 100 / (end - start)).toInt().coerceIn(0, 100)
        }

        private fun computeRemaining(
            endAt: String,
            zone: java.time.ZoneId,
        ): String {
            val end = parseIso(endAt, zone) ?: return ""
            val minutes = (end - System.currentTimeMillis()) / 60_000
            return formatRemaining(minutes.coerceAtLeast(0))
        }

        private fun formatUpdated(updatedAt: Long): String =
            if (updatedAt > 0L) {
                java.text
                    .SimpleDateFormat("HH:mm", java.util.Locale.getDefault())
                    .format(java.util.Date(updatedAt))
                    .let { "更新 $it" }
            } else {
                ""
            }

        private fun formatTimeRange(
            startAt: String?,
            endAt: String,
            zone: java.time.ZoneId,
        ): String {
            val start = startAt?.let { formatTime(it, zone) } ?: ""
            val end = formatTime(endAt, zone)
            return if (start.isNotEmpty()) {
                "$start 〜 $end"
            } else {
                end
            }
        }

        private fun formatRemaining(minutes: Long): String {
            val hours = minutes / 60
            val mins = minutes % 60
            return when {
                hours > 0 && mins > 0 -> "残 ${hours}時間${mins}分"
                hours > 0 -> "残 ${hours}時間"
                else -> "残 ${mins}分"
            }
        }
    }
}

private val ISO_FRACTIONAL_SECONDS = Regex("\\.\\d+")

private fun parseZone(serverTz: String?): java.time.ZoneId =
    if (serverTz != null) {
        try {
            java.time.ZoneId.of(serverTz)
        } catch (_: Exception) {
            java.time.ZoneId.systemDefault()
        }
    } else {
        java.time.ZoneId.systemDefault()
    }

private fun formatTime(
    iso: String?,
    zone: java.time.ZoneId = java.time.ZoneId.systemDefault(),
): String {
    val epoch = parseIso(iso, zone) ?: return ""
    return try {
        java.time.Instant
            .ofEpochMilli(epoch)
            .atZone(zone)
            .format(
                java.time.format.DateTimeFormatter
                    .ofPattern("HH:mm"),
            )
    } catch (_: Exception) {
        ""
    }
}

private fun parseIso(
    iso: String?,
    zone: java.time.ZoneId = java.time.ZoneId.systemDefault(),
): Long? {
    if (iso == null) return null
    return try {
        val s = iso.replace(ISO_FRACTIONAL_SECONDS, "").replace("Z", "+00:00")
        java.time.OffsetDateTime
            .parse(s)
            .toInstant()
            .toEpochMilli()
    } catch (e: Exception) {
        try {
            java.time.LocalDateTime
                .parse(iso.replace("Z", ""))
                .atZone(zone)
                .toInstant()
                .toEpochMilli()
        } catch (e2: Exception) {
            null
        }
    }
}
