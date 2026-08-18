package expo.modules.takususerver

import expo.modules.kotlin.exception.CodedException
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.records.Field
import expo.modules.kotlin.records.Record
import java.io.File
import org.json.JSONObject
import uniffi.takusu_android.ServerStatus
import uniffi.takusu_android.TakusuServer
import uniffi.takusu_android.clearLogs
import uniffi.takusu_android.getLogs
import uniffi.takusu_android.isModelCached
import uniffi.takusu_android.pushLog

class StartOptions : Record {
    @Field val port: Int = 3838

    @Field val workersUrl: String = ""

    @Field val rootToken: String = ""

    @Field val agentConfigJson: String = ""
}

class TakusuServerModule : Module() {
    private var server: TakusuServer? = null

    override fun definition() =
        ModuleDefinition {
            Name("TakusuServer")

            Function("startModelDownload") { modelId: String ->
                val context =
                    appContext.reactContext
                        ?: throw CodedException("ERR_NO_CONTEXT", "Android context is unavailable", null)
                ModelDownloadWorker.enqueue(context, modelId)
                true
            }

            AsyncFunction("isModelCached") { modelId: String ->
                val context =
                    appContext.reactContext
                        ?: throw CodedException("ERR_NO_CONTEXT", "Android context is unavailable", null)
                val modelRoot = File(context.noBackupFilesDir, "takusu/models")
                isModelCached(modelRoot.absolutePath, modelId)
            }

            Function("start") { options: StartOptions ->
                try {
                    if (server != null) {
                        throw CodedException("ERR_ALREADY_RUNNING", "Server already running", null)
                    }
                    val instance = TakusuServer()
                    instance.startWithAgentConfig(
                        options.port.toUShort(),
                        options.workersUrl,
                        options.rootToken,
                        options.agentConfigJson,
                    )
                    server = instance
                    true
                } catch (e: CodedException) {
                    throw e
                } catch (e: Exception) {
                    throw CodedException("ERR_START_FAILED", "Failed to start server: ${e.message}", e)
                }
            }

            Function("stop") {
                try {
                    val s = server ?: throw CodedException("ERR_NOT_RUNNING", "Server not running", null)
                    s.stop()
                    server = null
                    true
                } catch (e: CodedException) {
                    throw e
                } catch (e: Exception) {
                    throw CodedException("ERR_STOP_FAILED", "Failed to stop server: ${e.message}", e)
                }
            }

            Function("status") {
                try {
                    val s = server
                    if (s != null) {
                        when (val result = s.status()) {
                            is ServerStatus.Running -> mapOf("running" to true, "port" to result.port.toInt())
                            is ServerStatus.Stopped -> mapOf("running" to false, "port" to 0)
                        }
                    } else {
                        // A WorkManager worker may have started the server
                        // directly through `uniffi.takusu_android.TakusuServer`,
                        // so the module's `server` field can be null while the
                        // Rust registry still reports a live server.
                        when (val result = uniffi.takusu_android.globalServerStatus()) {
                            is ServerStatus.Running -> mapOf("running" to true, "port" to result.port.toInt())
                            is ServerStatus.Stopped -> mapOf("running" to false, "port" to 0)
                        }
                    }
                } catch (e: Exception) {
                    mapOf("running" to false, "port" to 0)
                }
            }

            Function("getLogs") {
                try {
                    getLogs()
                } catch (e: Exception) {
                    emptyList<String>()
                }
            }

            Function("clearLogs") {
                try {
                    clearLogs()
                    true
                } catch (e: Exception) {
                    false
                }
            }

            Function("pushLog") { line: String ->
                try {
                    pushLog(line)
                    true
                } catch (e: Exception) {
                    false
                }
            }

            Function("runScheduleOperation") {
                operation: String,
                operationId: String,
                paramsJson: String,
                workersUrl: String,
                token: String,
                port: Int,
                ->
                val context =
                    appContext.reactContext
                        ?: throw CodedException("ERR_NO_CONTEXT", "Android context is unavailable", null)
                if (workersUrl.isEmpty() || token.isEmpty()) {
                    throw CodedException("ERR_MISSING_CREDENTIALS", "workersUrl and token are required", null)
                }
                ScheduleOperationWorker.enqueue(
                    context,
                    operation,
                    operationId,
                    paramsJson,
                    workersUrl,
                    token,
                    port,
                )
                true
            }

            AsyncFunction("getScheduleOperationStatus") {
                val context = appContext.reactContext ?: return@AsyncFunction mapOf("status" to "none")
                val file = ScheduleOperationWorker.statusFile(context)
                if (!file.exists()) {
                    return@AsyncFunction mapOf("status" to "none")
                }
                try {
                    val json = JSONObject(file.readText())
                    mapOf(
                        "id" to json.optString("id", ""),
                        "status" to json.optString("status", "none"),
                        "operation" to json.optString("operation", ""),
                        "message" to json.optString("message", ""),
                    )
                } catch (e: Exception) {
                    mapOf("status" to "none")
                }
            }

            Function("clearScheduleOperationStatus") {
                val context = appContext.reactContext ?: return@Function false
                ScheduleOperationWorker.clearStatus(context)
                true
            }
        }

    companion object {
        init {
            try {
                System.loadLibrary("takusu_android")
            } catch (e: UnsatisfiedLinkError) {
                // Library not loaded yet — will be available after native build
            }
        }
    }
}
