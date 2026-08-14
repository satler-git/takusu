package expo.modules.takusuwidget

import android.content.Context
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.records.Field
import expo.modules.kotlin.records.Record
import org.json.JSONArray
import org.json.JSONObject

class WidgetConfig : Record {
    @Field val workersUrl: String = ""

    @Field val token: String = ""

    @Field val scheme: String? = null

    @Field val port: Int = 3838
}

class WidgetTask : Record {
    @Field val id: String = ""

    @Field val title: String = ""

    @Field val startAt: String? = null

    @Field val endAt: String = ""

    @Field val abandonability: Double = 0.75

    @Field val fixed: Boolean = false

    // WI-3: authority is a placeholder (candidate/today_covered) until WI-10.
    @Field val authority: String = "candidate"
}

class WidgetSnapshotInput : Record {
    @Field val doing: WidgetTask? = null

    @Field val upcoming: List<WidgetTask> = emptyList()

    @Field val unscheduledCount: Int = 0

    // WI-3: placeholders for the full resident-agent surface (WI-10/WI-18/WI-4).
    @Field val coverage: String = "bootstrap"

    @Field val settlement: String? = null

    @Field val capabilities: List<String> = emptyList()

    @Field val serverTz: String? = null

    @Field val scheme: String? = null
}

/**
 * Expo module that bridges JS -> native for the home screen widget.
 *
 * JS calls `saveConfig` to persist the Workers URL + token into
 * SharedPreferences (read by [WidgetUpdateWorker]).
 *
 * JS calls `saveSnapshot` to write the latest task data (fetched on the
 * JS side) into SharedPreferences and immediately refresh the widget,
 * so the widget shows fresh data without waiting for WorkManager.
 *
 * `requestUpdate` triggers a widget re-render from the cached snapshot.
 */
class TakusuWidgetModule : Module() {
    override fun definition() =
        ModuleDefinition {
            Name("TakusuWidget")

            Function("saveConfig") { config: WidgetConfig ->
                try {
                    val ctx = appContext.reactContext ?: return@Function false
                    val prefs = ctx.getSharedPreferences(WidgetUpdateWorker.PREFS_NAME, Context.MODE_PRIVATE)
                    val editor =
                        prefs
                            .edit()
                            .putString(WidgetUpdateWorker.KEY_WORKERS_URL, config.workersUrl)
                            .putString(WidgetUpdateWorker.KEY_TOKEN, config.token)
                            .putInt(WidgetUpdateWorker.KEY_PORT, config.port)
                    config.scheme?.takeIf { it.isNotEmpty() }?.let {
                        editor.putString(WidgetUpdateWorker.KEY_SCHEME, it)
                    }
                    editor.apply()
                    WidgetUpdateWorker.schedule(ctx)
                    true
                } catch (e: Exception) {
                    false
                }
            }

            Function("saveSnapshot") { input: WidgetSnapshotInput ->
                try {
                    val ctx = appContext.reactContext ?: return@Function false
                    val prefs = ctx.getSharedPreferences(WidgetUpdateWorker.PREFS_NAME, Context.MODE_PRIVATE)

                    val snap = JSONObject()
                    input.doing?.let { snap.put("doing", taskJson(it)) } ?: snap.put("doing", JSONObject.NULL)
                    snap.put("upcoming", JSONArray(input.upcoming.map { taskJson(it) }))
                    snap.put("unscheduled_count", input.unscheduledCount)
                    snap.put("coverage", input.coverage)
                    snap.put("settlement", input.settlement ?: JSONObject.NULL)
                    snap.put("capabilities", JSONArray(input.capabilities))
                    input.serverTz?.let { snap.put("server_tz", it) }
                    val scheme =
                        input.scheme?.takeIf { it.isNotEmpty() } ?: prefs.getString(WidgetUpdateWorker.KEY_SCHEME, null)
                    scheme?.let { snap.put("scheme", it) }

                    val editor =
                        prefs
                            .edit()
                            .putString(WidgetUpdateWorker.KEY_SNAPSHOT, snap.toString())
                            .putString(WidgetUpdateWorker.KEY_SERVER_TZ, input.serverTz)
                            .putLong(WidgetUpdateWorker.KEY_UPDATED_AT, System.currentTimeMillis())
                    scheme?.takeIf { it.isNotEmpty() }?.let { editor.putString(WidgetUpdateWorker.KEY_SCHEME, it) }
                    editor.apply()

                    TakusuWidgetProvider.updateWidget(ctx)
                    true
                } catch (e: Exception) {
                    false
                }
            }

            Function("requestUpdate") {
                try {
                    val ctx = appContext.reactContext ?: return@Function false
                    TakusuWidgetProvider.updateWidget(ctx)
                    true
                } catch (e: Exception) {
                    false
                }
            }
        }

    private fun taskJson(t: WidgetTask): JSONObject {
        val o = JSONObject()
        o.put("id", t.id)
        o.put("title", t.title)
        o.put("start_at", t.startAt ?: JSONObject.NULL)
        o.put("end_at", t.endAt)
        o.put("abandonability", t.abandonability)
        o.put("fixed", t.fixed)
        o.put("authority", t.authority)
        return o
    }
}
