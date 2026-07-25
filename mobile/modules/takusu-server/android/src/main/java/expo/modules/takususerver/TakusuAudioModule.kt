package expo.modules.takususerver

import android.content.Context
import android.media.AudioAttributes
import android.media.MediaPlayer
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import android.speech.tts.Voice
import android.util.Log
import expo.modules.kotlin.exception.CodedException
import expo.modules.kotlin.functions.Coroutine
import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import expo.modules.kotlin.records.Field
import expo.modules.kotlin.records.Record
import java.io.File
import java.util.Locale
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.takusu_android.MobileAudio

class AudioOptions : Record {
    @Field val provider: String = "cartesia"

    @Field val modelDir: String = ""

    @Field val apiKey: String = ""

    @Field val voiceId: String = ""

    @Field val language: String = "ja"

    @Field val sampleRate: Int = 44100

    @Field val speed: Double = 1.0

    @Field val mute: Boolean = false
}

private const val TAG = "TakusuAudioModule"

class TakusuAudioModule : Module() {
    private var audio: MobileAudio? = null
    private var recorder: AudioRecorder? = null

    @Volatile
    private var player: MediaPlayer? = null

    @Volatile
    private var textToSpeech: TextToSpeech? = null

    @Volatile
    private var ttsProvider: String = "cartesia"

    @Volatile
    private var muted: Boolean = false

    @Volatile
    private var pendingTtsCompletion: CompletableDeferred<Unit>? = null

    private val pendingTtsCompletions = ConcurrentHashMap<String, CompletableDeferred<Boolean>>()

    private val audioAttributes: AudioAttributes by lazy {
        AudioAttributes
            .Builder()
            .setUsage(AudioAttributes.USAGE_MEDIA)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
            .build()
    }

    private fun releasePlayer() {
        synchronized(this) {
            try {
                player?.release()
            } catch (_: Exception) {
                // Ignore release failures; the player may already be released or
                // in an invalid state.
            }
            player = null
            pendingTtsCompletion?.completeExceptionally(
                CodedException("ERR_TTS_INTERRUPTED", "TTS playback was interrupted", null),
            )
            pendingTtsCompletion = null
        }
    }

    private fun completeAllPendingTtsCompletions(result: Boolean) {
        val iterator = pendingTtsCompletions.iterator()
        while (iterator.hasNext()) {
            val (_, completion) = iterator.next()
            iterator.remove()
            completion.complete(result)
        }
    }

    override fun definition() =
        ModuleDefinition {
            Name("TakusuAudio")

            AsyncFunction("configure") Coroutine { options: AudioOptions ->
                val context =
                    appContext.reactContext
                        ?: throw CodedException("ERR_AUDIO_CONFIG", "React context is not available", null)

                // Release any active player and previous backend resources
                // before switching.
                releasePlayer()
                try {
                    audio?.shutdown()
                } catch (_: Exception) {
                    // ignore shutdown failures
                }
                audio = null
                completeAllPendingTtsCompletions(false)
                try {
                    textToSpeech?.stop()
                } catch (_: Exception) {
                    // ignore stop failures
                }
                try {
                    textToSpeech?.shutdown()
                } catch (_: Exception) {
                    // ignore shutdown failures
                }
                textToSpeech = null

                // Reset the provider choice; it will be restored only after the
                // new backend initializes successfully.
                ttsProvider = ""
                muted = options.mute

                when (options.provider) {
                    "android" -> {
                        val newAudio = createMobileAudio(context, options, apiKey = "")
                        val tts =
                            try {
                                initTextToSpeech(context)
                            } catch (error: Exception) {
                                try {
                                    newAudio.shutdown()
                                } catch (_: Exception) {
                                    // ignore shutdown failures
                                }
                                throw error
                            }
                        applyTtsOptions(tts, options)
                        textToSpeech = tts
                        audio = newAudio
                    }

                    "cartesia" -> {
                        audio = createMobileAudio(context, options, apiKey = options.apiKey)
                    }

                    else -> {
                        throw CodedException(
                            "ERR_TTS_UNSUPPORTED",
                            "Unsupported TTS provider: ${options.provider}",
                            null,
                        )
                    }
                }

                ttsProvider = options.provider
                true
            }

            AsyncFunction("setMuted") { muted: Boolean ->
                this@TakusuAudioModule.muted = muted
                audio?.setMuted(muted)
                true
            }

            Function("startRecording") {
                val instance = AudioRecorder()
                instance.start()
                recorder = instance
                true
            }

            AsyncFunction("stopAndTranscribe") {
                val samples =
                    recorder?.stop()
                        ?: throw CodedException("ERR_NOT_RECORDING", "Recording is not active", null)
                recorder = null
                val instance =
                    audio
                        ?: throw CodedException("ERR_AUDIO_CONFIG", "Audio is not configured", null)
                instance.transcribePcm(samples)
            }

            AsyncFunction("synthesizeAndPlay") Coroutine { text: String ->
                if (text.trim().isEmpty()) {
                    throw CodedException("ERR_TTS_EMPTY", "TTS text was empty", null)
                }

                if (ttsProvider.isEmpty()) {
                    throw CodedException(
                        "ERR_AUDIO_CONFIG",
                        "TTS provider is not configured",
                        null,
                    )
                }

                if (this@TakusuAudioModule.muted) {
                    return@Coroutine true
                }

                if (ttsProvider == "android") {
                    val tts =
                        textToSpeech
                            ?: throw CodedException("ERR_AUDIO_CONFIG", "Android TTS is not configured", null)
                    val completion = CompletableDeferred<Boolean>()
                    val utteranceId = UUID.randomUUID().toString()
                    pendingTtsCompletions[utteranceId] = completion
                    // Add to the queue instead of flushing so that multiple
                    // sequential synthesizeAndPlay calls are read in order.
                    val result = tts.speak(text, TextToSpeech.QUEUE_ADD, null, utteranceId)
                    if (result == TextToSpeech.ERROR) {
                        pendingTtsCompletions.remove(utteranceId)
                        throw CodedException("ERR_TTS_FAILED", "Android TTS speak failed", null)
                    }
                    try {
                        if (!completion.await()) {
                            throw CodedException("ERR_TTS_STOPPED", "TTS was interrupted or failed", null)
                        }
                    } finally {
                        pendingTtsCompletions.remove(utteranceId)
                    }
                    true
                } else {
                    val instance =
                        audio
                            ?: throw CodedException("ERR_AUDIO_CONFIG", "Audio is not configured", null)
                    val mp3 = instance.synthesize(text)
                    val cacheDir =
                        appContext.reactContext?.cacheDir
                            ?: throw CodedException("ERR_AUDIO_CONFIG", "React context is not available", null)
                    val file = File(cacheDir, "takusu-agent-response.mp3")
                    file.writeBytes(mp3)

                    // Do not release the player in an OnCompletionListener;
                    // releasing it as soon as completion fires can tear down the
                    // AudioTrack while it still has buffered frames, cutting off
                    // the end of mp3 playback. Release it on the next utterance,
                    // stopPlayback, configure, or OnDestroy instead.
                    releasePlayer()
                    val completion = CompletableDeferred<Unit>()
                    val mediaPlayer =
                        MediaPlayer().apply {
                            setAudioAttributes(audioAttributes)
                            setDataSource(file.absolutePath)
                        }
                    mediaPlayer.setOnPreparedListener {
                        synchronized(this@TakusuAudioModule) {
                            if (player === it) {
                                it.start()
                            }
                        }
                    }
                    mediaPlayer.setOnCompletionListener {
                        synchronized(this@TakusuAudioModule) {
                            pendingTtsCompletion?.complete(Unit)
                            pendingTtsCompletion = null
                        }
                    }
                    mediaPlayer.setOnErrorListener { _, what, extra ->
                        synchronized(this@TakusuAudioModule) {
                            pendingTtsCompletion?.completeExceptionally(
                                CodedException(
                                    "ERR_TTS_PLAYBACK",
                                    "MediaPlayer error during TTS playback: $what $extra",
                                    null,
                                ),
                            )
                            pendingTtsCompletion = null
                        }
                        true
                    }
                    synchronized(this@TakusuAudioModule) {
                        player = mediaPlayer
                        pendingTtsCompletion = completion
                    }
                    try {
                        mediaPlayer.prepareAsync()
                    } catch (error: Exception) {
                        releasePlayer()
                        throw CodedException(
                            "ERR_TTS_PLAYBACK",
                            "Failed to play TTS audio: ${error.message}",
                            error,
                        )
                    }
                    try {
                        completion.await()
                        true
                    } finally {
                        synchronized(this@TakusuAudioModule) {
                            if (pendingTtsCompletion === completion) {
                                pendingTtsCompletion = null
                            }
                        }
                    }
                }
            }

            Function("stopPlayback") {
                if (ttsProvider == "android") {
                    textToSpeech?.stop()
                    completeAllPendingTtsCompletions(false)
                } else {
                    releasePlayer()
                }
                true
            }

            AsyncFunction("getAvailableVoices") Coroutine { _: Any? ->
                val context =
                    appContext.reactContext
                        ?: throw CodedException("ERR_AUDIO_CONFIG", "React context is not available", null)

                val existingTts = textToSpeech
                val temporary = existingTts == null
                val tts =
                    existingTts
                        ?: try {
                            initTextToSpeech(context)
                        } catch (error: Exception) {
                            throw CodedException(
                                "ERR_TTS_INIT",
                                "Failed to initialize Android TTS to list voices: ${error.message}",
                                error,
                            )
                        }

                val voices =
                    try {
                        voicesFromTts(tts)
                    } finally {
                        if (temporary) {
                            try {
                                tts.shutdown()
                            } catch (_: Exception) {
                            }
                        }
                    }
                voices
            }

            OnDestroy {
                releasePlayer()
                try {
                    audio?.shutdown()
                } catch (_: Exception) {
                    // ignore shutdown failures
                }
                audio = null
                completeAllPendingTtsCompletions(false)
                try {
                    textToSpeech?.stop()
                } catch (_: Exception) {
                    // ignore stop failures
                }
                try {
                    textToSpeech?.shutdown()
                } catch (_: Exception) {
                    // ignore shutdown failures
                }
                textToSpeech = null
            }
        }

    private suspend fun initTextToSpeech(context: Context): TextToSpeech =
        withContext(Dispatchers.Main) {
            val initStatus = CompletableDeferred<Int>()
            val tts = TextToSpeech(context.applicationContext) { status -> initStatus.complete(status) }
            val status = initStatus.await()
            if (status == TextToSpeech.SUCCESS) {
                tts.setOnUtteranceProgressListener(
                    object : UtteranceProgressListener() {
                        override fun onStart(utteranceId: String?) {}

                        override fun onDone(utteranceId: String?) {
                            utteranceId?.let { pendingTtsCompletions.remove(it)?.complete(true) }
                        }

                        override fun onError(utteranceId: String?) {
                            utteranceId?.let { pendingTtsCompletions.remove(it)?.complete(false) }
                        }

                        override fun onStop(
                            utteranceId: String?,
                            interrupted: Boolean,
                        ) {
                            utteranceId?.let { pendingTtsCompletions.remove(it)?.complete(false) }
                        }

                        override fun onError(
                            utteranceId: String?,
                            errorCode: Int,
                        ) {
                            utteranceId?.let { pendingTtsCompletions.remove(it)?.complete(false) }
                        }
                    },
                )
                tts
            } else {
                try {
                    tts.shutdown()
                } catch (_: Exception) {
                }
                throw CodedException(
                    "ERR_TTS_INIT",
                    "Android TTS initialization failed with status $status",
                    null,
                )
            }
        }

    private fun createMobileAudio(
        context: Context,
        options: AudioOptions,
        apiKey: String,
    ): MobileAudio {
        val modelDir =
            options.modelDir.ifEmpty {
                File(context.noBackupFilesDir, "takusu/models").absolutePath
            }
        return try {
            MobileAudio(
                modelDir,
                apiKey,
                options.voiceId,
                options.language,
                options.sampleRate.toUInt(),
                options.speed.toFloat(),
                options.mute,
            )
        } catch (error: Exception) {
            throw CodedException(
                "ERR_AUDIO_CONFIG",
                "Failed to load audio models: ${error.message}",
                error,
            )
        }
    }

    private fun applyTtsOptions(
        tts: TextToSpeech,
        options: AudioOptions,
    ) {
        applyTtsOptions(tts, options.voiceId, options.language, options.speed.toFloat())
    }
}

internal fun applyTtsOptions(
    tts: TextToSpeech,
    voiceId: String,
    language: String,
    speed: Float,
) {
    val locale = Locale.forLanguageTag(language)
    val languageResult = tts.setLanguage(locale)
    if (languageResult == TextToSpeech.LANG_MISSING_DATA ||
        languageResult == TextToSpeech.LANG_NOT_SUPPORTED
    ) {
        Log.w(TAG, "TTS language '$language' is not supported, falling back to default locale")
        tts.setLanguage(Locale.getDefault())
    }

    val voice: Voice? =
        if (voiceId.isNotEmpty()) {
            tts.voices?.sortedBy { it.name }?.find { it.name == voiceId }
        } else {
            tts.voices?.sortedBy { it.name }?.firstOrNull()
        }
    if (voice != null) {
        val voiceResult = tts.setVoice(voice)
        if (voiceResult == TextToSpeech.ERROR) {
            Log.w(TAG, "Failed to set TTS voice '${voice.name}', continuing with engine default")
        }
    } else if (voiceId.isNotEmpty()) {
        Log.w(TAG, "TTS voice '$voiceId' not found, continuing with engine default")
    }

    tts.setSpeechRate(speed)
}

internal fun voicesFromTts(tts: TextToSpeech): List<Map<String, Any>> =
    tts.voices
        ?.sortedBy { it.name }
        ?.map { voice ->
            mapOf(
                "name" to voice.name,
                "locale" to (voice.locale?.toLanguageTag() ?: ""),
                "quality" to voice.quality,
                "latency" to voice.latency,
                "requiresNetworkConnection" to voice.isNetworkConnectionRequired,
                "features" to (voice.features?.toList() ?: emptyList<String>()),
            )
        } ?: emptyList()
