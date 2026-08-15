package expo.modules.takusuwidget

import java.net.HttpURLConnection
import java.net.URL
import org.json.JSONArray
import org.json.JSONObject

/**
 * Fetches the widget snapshot from the local Rust server.
 *
 * The local server is started by [WidgetUpdateWorker] before calling this.
 * Returns null on any failure so the caller can fall back to the cached
 * snapshot in SharedPreferences.
 */
data class UpcomingTask(
    val id: String,
    val title: String,
    val startAt: String?,
    val endAt: String,
    val abandonability: Double,
    val fixed: Boolean,
    // WI-3: authority is a placeholder (candidate/today_covered) until WI-10.
    val authority: String = "candidate",
)

data class WidgetSnapshot(
    val doing: UpcomingTask?,
    val upcoming: List<UpcomingTask>,
    val unscheduledCount: Int,
    // WI-3: placeholders for the full resident-agent surface (WI-10/WI-18/WI-4).
    val coverage: String = "bootstrap",
    val settlement: String? = null,
    val capabilities: List<String> = emptyList(),
)

object WidgetFetcher {
    private const val CONNECT_TIMEOUT_MS = 3_000
    private const val READ_TIMEOUT_MS = 5_000

    private fun baseUrl(port: Int): String = "http://127.0.0.1:$port"

    fun fetch(
        token: String,
        port: Int,
    ): WidgetSnapshot? {
        val base = baseUrl(port)
        return try {
            val tasks = fetchJsonArray("$base/api/tasks", token) ?: return null
            val schedule = fetchJsonObject("$base/api/schedule", token)
            val scheduleMap = parseScheduleMap(schedule)

            var doing: UpcomingTask? = null
            val upcoming = mutableListOf<UpcomingTask>()
            var unscheduled = 0

            for (i in 0 until tasks.length()) {
                val t = tasks.getJSONObject(i)
                val status = t.optString("status")
                when (status) {
                    "pending" -> {
                        unscheduled++
                    }

                    "scheduled", "in_progress" -> {
                        val task = buildTask(t, scheduleMap)
                        if (status == "in_progress" && doing == null) {
                            doing = task
                        } else {
                            upcoming.add(task)
                        }
                    }
                }
            }

            upcoming.sortBy { parseIso(it.startAt ?: it.endAt) ?: Long.MAX_VALUE }

            WidgetSnapshot(
                doing = doing,
                upcoming = upcoming,
                unscheduledCount = unscheduled,
                // WI-3: coverage / settlement / capabilities are placeholders;
                // the local API does not return them yet (WI-10 / WI-18 / WI-4).
                coverage = "bootstrap",
                settlement = null,
                capabilities = emptyList(),
            )
        } catch (e: Exception) {
            null
        }
    }

    private fun buildTask(
        t: JSONObject,
        scheduleMap: Map<String, Pair<String?, String>>,
    ): UpcomingTask {
        val id = t.getString("id")
        val entry = scheduleMap[id]
        val startAt = entry?.first ?: optStringOrNull(t, "start_at")
        val endAt = entry?.second ?: t.getString("end_at")
        val status = t.optString("status")
        val authority =
            when {
                status == "in_progress" -> {
                    "today_covered"
                }

                t.has("authority") -> {
                    t.optString("authority", "candidate").takeIf { it.isNotEmpty() } ?: "candidate"
                }

                else -> {
                    "candidate"
                }
            }
        return UpcomingTask(
            id = id,
            title = t.getString("title"),
            startAt = startAt,
            endAt = endAt,
            abandonability = t.optDouble("abandonability", 0.75),
            fixed = t.optBoolean("fixed", false),
            // WI-2: authority is derived from the task status. coverage,
            // settlement, and capabilities remain placeholders (WI-10/WI-18/WI-4).
            authority = authority,
        )
    }

    private fun fetchJsonArray(
        url: String,
        token: String,
    ): JSONArray? {
        val conn =
            (URL(url).openConnection() as HttpURLConnection).apply {
                connectTimeout = CONNECT_TIMEOUT_MS
                readTimeout = READ_TIMEOUT_MS
                setRequestProperty("Authorization", "Bearer $token")
            }
        return try {
            if (conn.responseCode != 200) return null
            val body = conn.inputStream.bufferedReader().use { it.readText() }
            JSONArray(body)
        } finally {
            conn.disconnect()
        }
    }

    private fun fetchJsonObject(
        url: String,
        token: String,
    ): JSONObject? {
        val conn =
            (URL(url).openConnection() as HttpURLConnection).apply {
                connectTimeout = CONNECT_TIMEOUT_MS
                readTimeout = READ_TIMEOUT_MS
                setRequestProperty("Authorization", "Bearer $token")
            }
        return try {
            if (conn.responseCode != 200) return null
            val body = conn.inputStream.bufferedReader().use { it.readText() }
            JSONObject(body)
        } finally {
            conn.disconnect()
        }
    }

    private fun parseScheduleMap(sched: JSONObject?): Map<String, Pair<String?, String>> {
        if (sched == null) return emptyMap()
        val map = mutableMapOf<String, Pair<String?, String>>()
        val scheduleStr = sched.optString("schedule", "")
        if (scheduleStr.isEmpty()) return map
        val entries = JSONArray(scheduleStr)
        for (i in 0 until entries.length()) {
            val e = entries.getJSONObject(i)
            val id = e.getString("task_id")
            val startAt = optStringOrNull(e, "start_at")
            val endAt = e.getString("end_at")
            map[id] = Pair(startAt, endAt)
        }
        return map
    }

    private fun optStringOrNull(
        obj: JSONObject,
        key: String,
    ): String? = if (obj.isNull(key) || !obj.has(key)) null else obj.optString(key, "")

    private fun parseIso(iso: String?): Long? {
        if (iso == null) return null
        return try {
            val s = iso.replace(Regex("\\.\\d+"), "").replace("Z", "+00:00")
            java.time.OffsetDateTime
                .parse(s)
                .toInstant()
                .toEpochMilli()
        } catch (e: Exception) {
            try {
                java.time.LocalDateTime
                    .parse(iso.replace("Z", ""))
                    .atZone(java.time.ZoneOffset.UTC)
                    .toInstant()
                    .toEpochMilli()
            } catch (e2: Exception) {
                null
            }
        }
    }
}
