package expo.modules.takusuwidget

import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.util.SizeF
import android.widget.RemoteViews
import androidx.core.widget.RemoteViewsCompat
import java.time.LocalDate
import java.time.ZonedDateTime
import java.time.format.DateTimeFormatter
import java.time.temporal.ChronoUnit
import java.util.Locale
import kotlin.math.abs
import org.json.JSONArray
import org.json.JSONObject

private const val TAG = "TakusuWidgetProvider"
private const val WIDGET_DAYS_HORIZON = 7

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
    // WI-3: placeholders for the full resident-agent surface (WI-10/WI-18/WI-4).
    val coverage: String?,
    val settlement: String?,
    val capabilities: List<String>,
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

            val size = resolveSize(context, widgetId, options)
            val layout =
                when (size) {
                    WidgetSize.W4x2 -> R.layout.takusu_widget_4x2
                    WidgetSize.W4x1 -> R.layout.takusu_widget_4x1
                    WidgetSize.W2x2 -> R.layout.takusu_widget_2x2
                    WidgetSize.W2x1 -> R.layout.takusu_widget_2x1
                }
            val views = RemoteViews(context.packageName, layout)
            WidgetTheme.apply(context, views)

            when (size) {
                WidgetSize.W4x2 -> render4x2(context, views, snapshot, updatedAt, widgetId, zone, scheme)
                WidgetSize.W4x1 -> render4x1(context, views, snapshot, updatedAt, zone, scheme)
                WidgetSize.W2x2 -> render2x2(context, views, snapshot, updatedAt, widgetId, zone, scheme)
                WidgetSize.W2x1 -> render2x1(context, views, snapshot, updatedAt, zone, scheme)
            }

            setClickIntents(context, views, size)
            manager.updateAppWidget(widgetId, views)
        }

        private fun resolveSize(
            context: Context,
            widgetId: Int,
            options: Bundle,
        ): WidgetSize {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                @Suppress("DEPRECATION")
                val sizes = options.getParcelableArrayList<SizeF>(AppWidgetManager.OPTION_APPWIDGET_SIZES)
                if (!sizes.isNullOrEmpty()) {
                    val current = pickCurrentSize(context, options, sizes)
                    return resolveSizeFromDimensions(
                        context,
                        widgetId,
                        current.width.toInt(),
                        current.height.toInt(),
                    )
                }
            }

            val (width, height) = currentSizeFromOptions(context, options)
            return resolveSizeFromDimensions(context, widgetId, width, height)
        }

        private fun pickCurrentSize(
            context: Context,
            options: Bundle,
            sizes: List<SizeF>,
        ): SizeF {
            val (targetWidth, targetHeight) = currentSizeFromOptions(context, options)
            return sizes.minByOrNull { abs(it.width - targetWidth) + abs(it.height - targetHeight) }
                ?: sizes.first()
        }

        private fun currentSizeFromOptions(
            context: Context,
            options: Bundle,
        ): Pair<Int, Int> {
            val landscape = context.resources.configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
            // The options bundle exposes MIN/MAX width/height as the bounds for the current
            // orientation. The conventional mapping is:
            //   portrait : MIN_WIDTH (narrower)  x MAX_HEIGHT (taller)
            //   landscape: MAX_WIDTH (wider)     x MIN_HEIGHT (shorter)
            // This gives the current orientation's dimensions even on pre-Android 12 devices
            // that do not report exact sizes via OPTION_APPWIDGET_SIZES.
            val width =
                if (landscape) {
                    options.getInt(AppWidgetManager.OPTION_APPWIDGET_MAX_WIDTH, 0)
                } else {
                    options.getInt(AppWidgetManager.OPTION_APPWIDGET_MIN_WIDTH, 0)
                }
            val height =
                if (landscape) {
                    options.getInt(AppWidgetManager.OPTION_APPWIDGET_MIN_HEIGHT, 0)
                } else {
                    options.getInt(AppWidgetManager.OPTION_APPWIDGET_MAX_HEIGHT, 0)
                }
            return width to height
        }

        private fun resolveSizeFromDimensions(
            context: Context,
            widgetId: Int,
            width: Int,
            height: Int,
        ): WidgetSize {
            val info =
                try {
                    AppWidgetManager.getInstance(context).getAppWidgetInfo(widgetId)
                } catch (_: Exception) {
                    null
                }
            val baseWidth = info?.minWidth ?: 250
            val baseHeight = info?.minHeight ?: 110
            val landscape = context.resources.configuration.orientation == Configuration.ORIENTATION_LANDSCAPE
            val wideTh = if (landscape) (baseWidth * 1.8).toInt() else (baseWidth * 0.8).toInt()
            val tallTh = if (landscape) baseHeight else (baseHeight * 1.3).toInt()
            val wide = width >= wideTh
            val tall = height >= tallTh
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
            val (items, futureCount) =
                buildCollectionItems(
                    context,
                    upcoming,
                    maxItems = 14,
                    overflowLayout = true,
                    zone = zone,
                    scheme = scheme,
                )
            val hasUpcoming = futureCount > 0
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
            val primary = primaryTask(snapshot, zone)
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

            val primary = primaryTask(snapshot, zone)
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
                    snapshot?.upcoming ?: emptyList()
                } else {
                    snapshot?.upcoming?.drop(1) ?: emptyList()
                }
            val (items, futureCount) =
                buildCollectionItems(
                    context,
                    extras,
                    maxItems = 3,
                    overflowLayout = false,
                    mini = true,
                    zone = zone,
                    scheme = scheme,
                )
            if (futureCount > 0) {
                views.setViewVisibility(R.id.widget_mini_list, android.view.View.VISIBLE)
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
            val primary = primaryTask(snapshot, zone)
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

        private data class CollectionResult(
            val items: RemoteViewsCompat.RemoteCollectionItems,
            val futureCount: Int,
        )

        private fun buildCollectionItems(
            context: Context,
            tasks: List<UpcomingTask>,
            maxItems: Int,
            overflowLayout: Boolean,
            mini: Boolean = false,
            zone: java.time.ZoneId,
            scheme: String,
        ): CollectionResult {
            val viewTypeCount = 1 + (if (!mini) 1 else 0) + (if (overflowLayout) 1 else 0)
            val builder =
                RemoteViewsCompat.RemoteCollectionItems
                    .Builder()
                    .setViewTypeCount(viewTypeCount)
                    .setHasStableIds(false)

            val today = ZonedDateTime.now(zone).toLocalDate()
            val horizon = today.plusDays(WIDGET_DAYS_HORIZON.toLong())
            val datedTasks =
                tasks
                    .mapNotNull { task ->
                        val date = parseDate(task.startAt ?: task.endAt, zone) ?: return@mapNotNull null
                        if (date.isBefore(today)) {
                            null
                        } else {
                            task to date
                        }
                    }.sortedBy { it.second }

            val withinHorizon = datedTasks.filter { it.second <= horizon }
            val shown =
                if (withinHorizon.size >= maxItems) {
                    withinHorizon.take(maxItems)
                } else {
                    withinHorizon + datedTasks.filter { it.second > horizon }.take(maxItems - withinHorizon.size)
                }

            var itemId = 0L
            var lastDate: LocalDate? = null
            for ((task, date) in shown) {
                if (!mini) {
                    if (date != lastDate) {
                        builder.addItem(
                            itemId++,
                            buildDateHeaderRemoteViews(context, date, today),
                        )
                        lastDate = date
                    }
                }
                builder.addItem(
                    itemId++,
                    buildItemRemoteViews(context, task, mini, zone, scheme),
                )
            }
            val overflow = datedTasks.size - shown.size
            if (overflowLayout && overflow > 0) {
                builder.addItem(
                    itemId,
                    buildOverflowRemoteViews(context, overflow, scheme),
                )
            }
            return CollectionResult(builder.build(), datedTasks.size)
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

        private fun buildDateHeaderRemoteViews(
            context: Context,
            date: LocalDate,
            today: LocalDate,
        ): RemoteViews {
            val views = RemoteViews(context.packageName, R.layout.takusu_widget_date_header)
            views.setTextViewText(R.id.widget_date_header_text, formatDateLabel(date, today))
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

        private fun primaryTask(
            snapshot: Snapshot?,
            zone: java.time.ZoneId,
        ): UpcomingTask? {
            if (snapshot == null) return null
            // Prefer the task currently in progress; otherwise use the first task that is
            // today or in the future. `snapshot.upcoming` may contain scheduled tasks whose
            // start time is in the past, and those are intentionally skipped here.
            val today = ZonedDateTime.now(zone).toLocalDate()
            return snapshot.doing ?: snapshot.upcoming.firstOrNull { task ->
                val date = parseDate(task.startAt ?: task.endAt, zone)
                date != null && !date.isBefore(today)
            }
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
                coverage = optStringOrNull(obj, "coverage") ?: "bootstrap",
                settlement = optStringOrNull(obj, "settlement"),
                capabilities = parseStringArray(obj.optJSONArray("capabilities")),
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
                    authority = obj.optString("authority", "candidate"),
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
                authority = "candidate",
            )
        }

        private fun optStringOrNull(
            obj: JSONObject,
            key: String,
        ): String? = if (obj.isNull(key) || !obj.has(key)) null else obj.optString(key, "")

        private fun parseStringArray(arr: JSONArray?): List<String> {
            if (arr == null) return emptyList()
            val result = mutableListOf<String>()
            for (i in 0 until arr.length()) {
                result.add(arr.optString(i, ""))
            }
            return result.filter { it.isNotEmpty() }
        }

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

private fun parseDate(
    iso: String?,
    zone: java.time.ZoneId = java.time.ZoneId.systemDefault(),
): LocalDate? {
    val epoch = parseIso(iso, zone) ?: return null
    return try {
        java.time.Instant
            .ofEpochMilli(epoch)
            .atZone(zone)
            .toLocalDate()
    } catch (_: Exception) {
        null
    }
}

private fun formatDateLabel(
    date: LocalDate,
    today: LocalDate,
): String {
    val days = ChronoUnit.DAYS.between(today, date)
    return when (days) {
        0L -> "今日"
        1L -> "明日"
        else -> DateTimeFormatter.ofPattern("M/d").withLocale(Locale.getDefault()).format(date)
    }
}
