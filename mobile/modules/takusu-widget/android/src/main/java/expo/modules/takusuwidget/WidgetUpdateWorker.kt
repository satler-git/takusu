package expo.modules.takusuwidget

import android.content.Context
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

/**
 * Periodic worker that refreshes the widget.
 *
 * It tries to fetch from the local Rust server at the port persisted in
 * SharedPreferences. If the server is not running, it starts it via the
 * UniFFI bindings, fetches, then stops it. The fetched snapshot is persisted
 * to SharedPreferences and the widget is updated.
 */
class WidgetUpdateWorker(
    context: Context,
    params: WorkerParameters,
) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val prefs = applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val token = prefs.getString(KEY_TOKEN, null)
        val workersUrl = prefs.getString(KEY_WORKERS_URL, null)
        val port = prefs.getInt(KEY_PORT, DEFAULT_PORT)
        val serverTz = prefs.getString(KEY_SERVER_TZ, null)
        val scheme = prefs.getString(KEY_SCHEME, null)

        if (token == null || workersUrl == null) {
            persistEmptySnapshot(prefs)
            TakusuWidgetProvider.updateWidget(applicationContext)
            return Result.success()
        }

        var snapshot = withContext(Dispatchers.IO) { WidgetFetcher.fetch(token, port) }

        if (snapshot == null) {
            snapshot = startServerAndFetch(workersUrl, token, port)
        }

        if (snapshot != null) {
            persistSnapshot(prefs, snapshot, serverTz, scheme)
            TakusuWidgetProvider.updateWidget(applicationContext)
            return Result.success()
        }

        return if (runAttemptCount + 1 >= MAX_ATTEMPTS) Result.failure() else Result.retry()
    }

    private suspend fun startServerAndFetch(
        workersUrl: String,
        token: String,
        port: Int,
    ): WidgetSnapshot? =
        withContext(Dispatchers.IO) {
            try {
                val server = uniffi.takusu_android.TakusuServer()
                try {
                    server.start(port.toUShort(), workersUrl, token)
                    delay(500)
                    WidgetFetcher.fetch(token, port)
                } finally {
                    try {
                        server.stop()
                    } catch (_: Exception) {
                    }
                    try {
                        server.destroy()
                    } catch (_: Exception) {
                    }
                }
            } catch (e: Exception) {
                null
            }
        }

    companion object {
        const val PREFS_NAME = "takusu_widget"
        const val KEY_TOKEN = "token"
        const val KEY_WORKERS_URL = "workers_url"
        const val KEY_PORT = "port"
        const val KEY_SNAPSHOT = "snapshot_json"
        const val KEY_SERVER_TZ = "server_tz"
        const val KEY_SCHEME = "scheme"
        const val KEY_UPDATED_AT = "updated_at"
        const val WORK_NAME = "takusu_widget_update"
        private const val DEFAULT_PORT = 3838
        private const val MAX_ATTEMPTS = 3

        fun persistSnapshot(
            prefs: android.content.SharedPreferences,
            s: WidgetSnapshot,
            serverTz: String? = null,
            scheme: String? = null,
        ) {
            val snap = JSONObject()
            snap.put("doing", s.doing?.let { taskJson(it) } ?: JSONObject.NULL)
            snap.put("upcoming", JSONArray(s.upcoming.map { taskJson(it) }))
            snap.put("unscheduled_count", s.unscheduledCount)
            snap.put("coverage", s.coverage)
            snap.put("settlement", s.settlement ?: JSONObject.NULL)
            snap.put("capabilities", JSONArray(s.capabilities))
            serverTz?.let { snap.put("server_tz", it) }
            scheme?.takeIf { it.isNotEmpty() }?.let { snap.put("scheme", it) }

            val editor =
                prefs
                    .edit()
                    .putString(KEY_SNAPSHOT, snap.toString())
                    .putString(KEY_SERVER_TZ, serverTz)
                    .putLong(KEY_UPDATED_AT, System.currentTimeMillis())
            scheme?.takeIf { it.isNotEmpty() }?.let { editor.putString(KEY_SCHEME, it) }
            editor.apply()
        }

        private fun taskJson(t: UpcomingTask): JSONObject {
            val o = JSONObject()
            o.put("id", t.id)
            o.put("title", t.title)
            o.put("start_at", t.startAt ?: JSONObject.NULL)
            o.put("end_at", t.endAt)
            o.put("abandonability", t.abandonability)
            o.put("fixed", t.fixed)
            // WI-3: authority is a placeholder until the local API returns it (WI-10).
            o.put("authority", t.authority)
            return o
        }

        private fun persistEmptySnapshot(prefs: android.content.SharedPreferences) {
            prefs
                .edit()
                .remove(KEY_SNAPSHOT)
                .remove(KEY_UPDATED_AT)
                .apply()
        }

        fun schedule(context: Context) {
            val constraints =
                Constraints
                    .Builder()
                    .setRequiredNetworkType(NetworkType.CONNECTED)
                    .build()
            val request =
                PeriodicWorkRequestBuilder<WidgetUpdateWorker>(30, TimeUnit.MINUTES)
                    .setConstraints(constraints)
                    .build()
            WorkManager.getInstance(context).enqueueUniquePeriodicWork(
                WORK_NAME,
                ExistingPeriodicWorkPolicy.KEEP,
                request,
            )
        }

        fun cancel(context: Context) {
            WorkManager.getInstance(context).cancelUniqueWork(WORK_NAME)
        }
    }
}
