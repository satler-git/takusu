package expo.modules.takusuagentservice

import android.content.Context
import android.os.Build
import android.util.Log
import androidx.core.content.ContextCompat
import expo.modules.kotlin.exception.CodedException
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.records.Field
import expo.modules.kotlin.records.Record

class AmbientStartOptions : Record {
    @Field val workersUrl: String = ""

    @Field val rootToken: String = ""

    @Field val deviceId: String = ""

    @Field val localUrl: String = ""

    @Field val modelDir: String = ""

    @Field val asrModel: String = "sherpa-sense-voice-int8"

    @Field val language: String = "ja"

    @Field val wakeWordBackend: String = "sherpa_kws"
}

class TakusuAgentServiceModule : Module() {
    override fun definition() =
        ModuleDefinition {
            Name("TakusuAgentService")

            Function("startAmbient") { options: AmbientStartOptions ->
                val context =
                    appContext.reactContext
                        ?: throw CodedException("ERR_NO_CONTEXT", "Android コンテキストが取得できません", null)

                startAmbient(context, options)
            }

            Function("stopAmbient") {
                val context =
                    appContext.reactContext
                        ?: throw CodedException("ERR_NO_CONTEXT", "Android コンテキストが取得できません", null)

                stopAmbient(context)
            }

            Function("isRunning") {
                TakusuAgentService.isRunning
            }

            Function("setAmbientEnabled") { enabled: Boolean ->
                val context =
                    appContext.reactContext
                        ?: throw CodedException("ERR_NO_CONTEXT", "Android コンテキストが取得できません", null)

                AmbientPreferences(context).setAmbientEnabled(enabled)
                true
            }

            Function("isAmbientEnabled") {
                val context =
                    appContext.reactContext
                        ?: throw CodedException("ERR_NO_CONTEXT", "Android コンテキストが取得できません", null)

                AmbientPreferences(context).isAmbientEnabled()
            }
        }

    private fun startAmbient(
        context: Context,
        options: AmbientStartOptions,
    ): Boolean {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M &&
            ContextCompat.checkSelfPermission(context, android.Manifest.permission.RECORD_AUDIO) !=
            android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            throw CodedException(
                "ERR_MISSING_PERMISSION",
                "マイクの録音権限が必要です",
                null,
            )
        }

        val prefs = AmbientPreferences(context)
        prefs.setAmbientEnabled(true)
        prefs.setStartOptions(
            workersUrl = options.workersUrl,
            rootToken = options.rootToken,
            deviceId = options.deviceId.ifBlank { AmbientPreferences.DEFAULT_DEVICE_ID },
            localUrl = options.localUrl,
            modelDir = options.modelDir,
            asrModel = options.asrModel,
            language = options.language,
            wakeWordBackend = options.wakeWordBackend.ifBlank { AmbientPreferences.DEFAULT_WAKE_WORD_BACKEND },
        )

        try {
            ContextCompat.startForegroundService(context, TakusuAgentService.startIntent(context))
        } catch (e: Throwable) {
            Log.e("TakusuAgentServiceModule", "failed to start ambient service", e)
            throw CodedException(
                "ERR_START_FAILED",
                "常時聞き取りを開始できませんでした: ${e.message}",
                e,
            )
        }

        return true
    }

    private fun stopAmbient(context: Context): Boolean {
        context.stopService(TakusuAgentService.stopIntent(context))
        return true
    }
}
