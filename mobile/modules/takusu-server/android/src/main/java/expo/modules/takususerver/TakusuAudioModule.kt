package expo.modules.takususerver

import android.content.Context
import android.media.AudioAttributes
import android.media.MediaPlayer
import android.os.Bundle
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
import uniffi.takusu_android.AndroidVad
import uniffi.takusu_android.MobileAudio
import uniffi.takusu_android.MobileSpeaker

class AudioOptions : Record {
    @Field val provider: String = "cartesia"

    @Field val modelDir: String = ""

    @Field val model: String = ""

    @Field val asrModel: String = "sherpa-sense-voice-int8"

    @Field val apiKey: String = ""

    @Field val voiceId: String = ""

    @Field val language: String = "ja"

    @Field val sampleRate: Int = 44100

    @Field val speed: Double = 1.0

    @Field val mute: Boolean = false
}

class SpeakerOptions : Record {
    @Field val modelDir: String = ""

    @Field val voiceDir: String = ""

    @Field val threshold: Double = 0.5
}

private const val TAG = "TakusuAudioModule"

/** Convert 16-bit PCM samples to 32-bit float [-1.0, 1.0]. */
private fun List<Short>.toFloatList(): List<Float> = map { it.toFloat() / 32768.0f }

class TakusuAudioModule : Module() {
    private var audio: MobileAudio? = null
    private var recorder: AudioRecorder? = null

    /** Reusable VAD endpoint. Loaded once and reset before each recording. */
    private var vad: AndroidVad? = null

    /** Speaker verifier. Created lazily when the speaker settings are configured. */
    private var speaker: MobileSpeaker? = null

    /** Recorder used for speaker enrollment / verification samples. */
    private var speakerRecorder: AudioRecorder? = null

    @Volatile
    private var player: MediaPlayer? = null

    @Volatile
    private var textToSpeech: TextToSpeech? = null

    @Volatile
    private var ttsProvider: String = "cartesia"

    @Volatile
    private var audioLanguage: String = "ja"

    @Volatile
    private var muted: Boolean = false

    @Volatile
    private var pendingTtsCompletion: CompletableDeferred<Unit>? = null

    @Volatile
    private var stopTtsRequested: Boolean = false

    private val pendingTtsCompletions = ConcurrentHashMap<String, CompletableDeferred<Boolean>>()

    private val cacheDir: File?
        get() = appContext.reactContext?.cacheDir

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

    private suspend fun ensureVad() {
        val dir =
            cacheDir
                ?: throw CodedException("ERR_AUDIO_CONFIG", "React context is not available", null)
        if (vad == null) {
            val modelDir =
                withIoContext {
                    uniffi.takusu_android.downloadModel(
                        dir.absolutePath,
                        "silero-vad",
                        "${dir.absolutePath}/silero-vad-status.json",
                    )
                }
            vad = AndroidVad("$modelDir/silero_vad.onnx")
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
                audioLanguage = options.language
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

                    "cartesia", "fish" -> {
                        if (options.apiKey.isBlank()) {
                            throw CodedException(
                                "ERR_AUDIO_CONFIG",
                                "${options.provider} requires an API key",
                                null,
                            )
                        }
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

            AsyncFunction("startRecording") {
                val instance =
                    audio
                        ?: throw CodedException("ERR_AUDIO_CONFIG", "Audio is not configured", null)
                val newRecorder = AudioRecorder()
                newRecorder.startStreaming(instance, audioLanguage)
                recorder = newRecorder
                true
            }

            // VAD endpointing: downloads the Silero model (first run) and stops
            // recording ~0.5 s after speech ends instead of requiring a tap.
            AsyncFunction("startRecordingWithEndpointing") Coroutine {
                ensureVad()
                val instance = AudioRecorder()
                instance.setVadEndpointing(vad)
                instance.start()
                recorder = instance
                true
            }

            AsyncFunction("stopAndTranscribe") {
                val activeRecorder =
                    recorder
                        ?: throw CodedException("ERR_NOT_RECORDING", "Recording is not active", null)
                recorder = null
                if (activeRecorder.isStreaming()) {
                    activeRecorder.stopStreaming()
                } else {
                    val instance =
                        audio
                            ?: throw CodedException("ERR_AUDIO_CONFIG", "Audio is not configured", null)
                    val samples = activeRecorder.stop()
                    instance.transcribePcm(samples)
                }
            }

            AsyncFunction("stopAndGetPcm") {
                val activeRecorder =
                    recorder
                        ?: throw CodedException("ERR_NOT_RECORDING", "Recording is not active", null)
                recorder = null
                if (activeRecorder.isStreaming()) {
                    throw CodedException(
                        "ERR_NOT_RECORDING",
                        "Recording is in streaming mode; stopAndGetPcm only works for raw PCM capture",
                        null,
                    )
                }
                activeRecorder.stop()
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

                stopTtsRequested = false

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
                    val mp3 =
                        withIoContext {
                            instance.synthesize(text)
                        }
                    val dir =
                        cacheDir ?: throw CodedException("ERR_AUDIO_CONFIG", "React context is not available", null)
                    val file = File(dir, "takusu-agent-response.mp3")
                    file.writeBytes(mp3)
                    playMp3File(file, deleteOnComplete = false)
                    true
                }
            }

            AsyncFunction("synthesizeToFile") Coroutine { text: String ->
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
                    return@Coroutine ""
                }

                if (stopTtsRequested) {
                    throw CodedException(
                        "ERR_TTS_INTERRUPTED",
                        "TTS synthesis was interrupted",
                        null,
                    )
                }

                val dir = cacheDir ?: throw CodedException("ERR_AUDIO_CONFIG", "React context is not available", null)

                if (ttsProvider == "android") {
                    val tts =
                        textToSpeech
                            ?: throw CodedException("ERR_AUDIO_CONFIG", "Android TTS is not configured", null)
                    val file = File.createTempFile("takusu-tts-", ".wav", dir)
                    val completion = CompletableDeferred<Boolean>()
                    val utteranceId = UUID.randomUUID().toString()
                    pendingTtsCompletions[utteranceId] = completion
                    val result = tts.synthesizeToFile(text, Bundle(), file, utteranceId)
                    if (result == TextToSpeech.ERROR) {
                        pendingTtsCompletions.remove(utteranceId)
                        file.delete()
                        throw CodedException("ERR_TTS_FAILED", "Android TTS synthesizeToFile failed", null)
                    }
                    try {
                        if (!completion.await()) {
                            file.delete()
                            throw CodedException("ERR_TTS_STOPPED", "TTS was interrupted or failed", null)
                        }
                    } finally {
                        pendingTtsCompletions.remove(utteranceId)
                    }
                    file.absolutePath
                } else {
                    val instance =
                        audio
                            ?: throw CodedException("ERR_AUDIO_CONFIG", "Audio is not configured", null)
                    val file = File.createTempFile("takusu-tts-", ".mp3", dir)
                    try {
                        val mp3 =
                            withIoContext {
                                instance.synthesize(text)
                            }
                        if (stopTtsRequested) {
                            file.delete()
                            throw CodedException(
                                "ERR_TTS_INTERRUPTED",
                                "TTS synthesis was interrupted",
                                null,
                            )
                        }
                        file.writeBytes(mp3)
                        if (stopTtsRequested) {
                            file.delete()
                            throw CodedException(
                                "ERR_TTS_INTERRUPTED",
                                "TTS synthesis was interrupted",
                                null,
                            )
                        }
                        file.absolutePath
                    } catch (e: CodedException) {
                        file.delete()
                        throw e
                    } catch (e: Throwable) {
                        file.delete()
                        throw CodedException(
                            "ERR_TTS_FAILED",
                            "TTS synthesis failed: ${e.message}",
                            e,
                        )
                    }
                }
            }

            AsyncFunction("playFile") Coroutine { path: String ->
                val file = File(path)
                if (!file.exists()) {
                    throw CodedException("ERR_TTS_PLAYBACK", "Audio file not found: $path", null)
                }
                playMp3File(file, deleteOnComplete = true)
                true
            }

            AsyncFunction("deleteFile") { path: String ->
                val file = File(path)
                val dir = cacheDir
                if (dir != null && file.parent == dir.absolutePath && file.exists()) {
                    file.delete()
                }
                true
            }

            Function("stopPlayback") {
                if (ttsProvider == "android") {
                    textToSpeech?.stop()
                    completeAllPendingTtsCompletions(false)
                } else {
                    stopTtsRequested = true
                    releasePlayer()
                }
                true
            }

            Function("clearTtsStop") {
                stopTtsRequested = false
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

            AsyncFunction("configureSpeaker") Coroutine { options: SpeakerOptions ->
                withIoContext {
                    val context =
                        appContext.reactContext
                            ?: throw CodedException("ERR_AUDIO_CONFIG", "React context is not available", null)
                    val modelDir =
                        options.modelDir.ifEmpty {
                            File(context.noBackupFilesDir, "takusu/models").absolutePath
                        }
                    val voiceDir =
                        options.voiceDir.ifEmpty {
                            File(context.noBackupFilesDir, "takusu/voiceprint").absolutePath
                        }
                    try {
                        speaker = MobileSpeaker(modelDir, voiceDir, options.threshold.toFloat())
                    } catch (error: Exception) {
                        throw CodedException(
                            "ERR_SPEAKER_CONFIG",
                            "Failed to load speaker verifier: ${error.message}",
                            error,
                        )
                    }
                }
                true
            }

            AsyncFunction("startSpeakerRecording") {
                if (speakerRecorder?.let { it.isRunning() } == true) {
                    throw CodedException("ERR_SPEAKER_RECORDING", "Speaker recording is already active", null)
                }
                val instance = AudioRecorder()
                instance.start()
                speakerRecorder = instance
                true
            }

            AsyncFunction("stopAndEnrollSpeaker") Coroutine { name: String ->
                val activeRecorder =
                    speakerRecorder
                        ?: throw CodedException("ERR_NOT_RECORDING", "Speaker recording is not active", null)
                speakerRecorder = null
                val samples = activeRecorder.stop()
                val verifier =
                    speaker
                        ?: throw CodedException("ERR_SPEAKER_CONFIG", "Speaker verifier is not configured", null)
                withIoContext {
                    try {
                        verifier.enroll(name, samples.toFloatList())
                    } catch (error: Throwable) {
                        throw CodedException(
                            "ERR_SPEAKER_ENROLL",
                            "Speaker enrollment failed: ${error.message}",
                            error,
                        )
                    }
                }
                true
            }

            AsyncFunction("stopAndVerifySpeaker") Coroutine { name: String ->
                val activeRecorder =
                    speakerRecorder
                        ?: throw CodedException("ERR_NOT_RECORDING", "Speaker recording is not active", null)
                speakerRecorder = null
                val samples = activeRecorder.stop()
                val verifier =
                    speaker
                        ?: throw CodedException("ERR_SPEAKER_CONFIG", "Speaker verifier is not configured", null)
                withIoContext {
                    try {
                        val result = verifier.verify(name, samples.toFloatList())
                        mapOf(
                            "score" to result.score.toDouble(),
                            "accepted" to result.accepted,
                            "speaker" to result.speaker,
                        )
                    } catch (error: Throwable) {
                        throw CodedException(
                            "ERR_SPEAKER_VERIFY",
                            "Speaker verification failed: ${error.message}",
                            error,
                        )
                    }
                }
            }

            AsyncFunction("deleteSpeaker") Coroutine { name: String ->
                val verifier =
                    speaker
                        ?: throw CodedException("ERR_SPEAKER_CONFIG", "Speaker verifier is not configured", null)
                withIoContext {
                    try {
                        verifier.delete(name)
                    } catch (error: Throwable) {
                        throw CodedException(
                            "ERR_SPEAKER_DELETE",
                            "Failed to delete speaker: ${error.message}",
                            error,
                        )
                    }
                }
                true
            }

            AsyncFunction("listSpeakers").Coroutine<Unit> {
                val verifier =
                    speaker
                        ?: throw CodedException("ERR_SPEAKER_CONFIG", "Speaker verifier is not configured", null)
                withIoContext {
                    verifier.list()
                }
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

    private suspend fun playMp3File(
        file: File,
        deleteOnComplete: Boolean,
    ) {
        try {
            if (stopTtsRequested) {
                throw CodedException(
                    "ERR_TTS_INTERRUPTED",
                    "TTS playback was interrupted",
                    null,
                )
            }
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
            } finally {
                synchronized(this@TakusuAudioModule) {
                    if (pendingTtsCompletion === completion) {
                        pendingTtsCompletion = null
                    }
                }
            }
        } finally {
            if (deleteOnComplete) {
                file.delete()
            }
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

    /**
     * Wrap [withContext] in a non-inline suspend helper so it can be called from
     * [expo.modules.kotlin.functions.Coroutine] blocks. The [Coroutine] body is
     * crossinline and cannot contain direct calls to inline suspend functions
     * such as [withContext] with newer Kotlin compilers.
     */
    private suspend fun <T> withIoContext(block: suspend () -> T): T = withContext(Dispatchers.IO) { block() }

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
            val audio =
                MobileAudio(
                    modelDir,
                    options.provider,
                    apiKey,
                    options.model,
                    options.voiceId,
                    options.language,
                    options.sampleRate.toUInt(),
                    options.speed.toFloat(),
                    options.mute,
                )
            audio.setAsrModel(options.asrModel)
            audio
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
