package expo.modules.takusuagentservice

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.telephony.PhoneStateListener
import android.telephony.TelephonyCallback
import android.telephony.TelephonyManager
import android.util.Log
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import expo.modules.takususerver.MicrophoneCoordinator
import java.io.File
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.locks.ReentrantLock
import uniffi.takusu_android.AmbientCallback
import uniffi.takusu_android.MobileAmbient

private val runningRef = AtomicBoolean(false)

class TakusuAgentService : Service() {
    private val mainHandler = Handler(Looper.getMainLooper())

    private val lifecycleLock = ReentrantLock()

    @Volatile
    private var mobileAmbient: MobileAmbient? = null

    @Volatile
    private var audioRecord: AudioRecord? = null

    @Volatile
    private var recordingThread: Thread? = null

    @Volatile
    private var audioManager: AudioManager? = null

    @Volatile
    private var telephonyManager: TelephonyManager? = null

    @Volatile
    private var phoneStateListener: PhoneStateListener? = null

    @Volatile
    private var telephonyCallback: TelephonyCallback? = null

    @Volatile
    private var audioFocusRequest: AudioFocusRequest? = null

    private val paused = AtomicBoolean(false)
    private val audioFocusPaused = AtomicBoolean(false)
    private val callPaused = AtomicBoolean(false)
    private val hasError = AtomicBoolean(false)

    @Volatile
    private var currentState = "起動中"

    private val ambientCallback: AmbientCallback =
        object : AmbientCallback {
            override fun onListening() {
                updateState("待機中")
                // Do not hold audio focus while only listening; other apps
                // should not be paused for long periods of wake-word watching.
                mainHandler.post { abandonAudioFocus() }
            }

            override fun onTranscribing() {
                updateState("文字起こし中")
            }

            override fun onWakeWord(text: String) {
                updateState("「$text」")
                // The user is about to speak a command. Request transient focus
                // so short media playback is ducked rather than fully paused.
                mainHandler.post { requestAudioFocusForCommand() }
            }

            override fun onResult(
                text: String,
                samples: List<Float>,
            ) {
                // Intentionally post a notification for now. Sending the
                // transcript to the local evaluator requires a dedicated
                // ambient-text input endpoint on takusu-agent (e.g. a new
                // /surface/text route or a session-less turn); the localUrl,
                // rootToken, and workersUrl are already stored in
                // AmbientPreferences for when that endpoint is available.
                // TODO(WI-23): wire the onResult transcript to the evaluator.
                updateState(text)
                mainHandler.post {
                    abandonAudioFocus()
                    AmbientNotificationHelper.postResultNotification(this@TakusuAgentService, text)
                }
            }

            override fun onError(error: String) {
                hasError.set(true)
                updateState("エラー: $error")
                AmbientPreferences(this@TakusuAgentService).setAmbientEnabled(false)
                mainHandler.post {
                    abandonAudioFocus()
                    if (runningRef.get()) {
                        stopSelf()
                    }
                }
            }

            override fun onStopped() {
                // A Rust-side stop normally follows onError or onDestroy
                // cleanup. If an error occurred, keep the error state visible
                // and do not overwrite it with "stopped".
                if (hasError.get()) {
                    return
                }
                updateState("停止")
                mainHandler.post {
                    abandonAudioFocus()
                    if (runningRef.get()) {
                        stopSelf()
                    }
                }
            }
        }

    private val audioFocusChangeListener =
        AudioManager.OnAudioFocusChangeListener { focusChange ->
            when (focusChange) {
                AudioManager.AUDIOFOCUS_GAIN -> {
                    audioFocusPaused.set(false)
                    updatePaused()
                }

                else -> {
                    audioFocusPaused.set(true)
                    updatePaused()
                }
            }
        }

    init {
        try {
            System.loadLibrary("takusu_android")
        } catch (_: UnsatisfiedLinkError) {
            // Library not loaded yet — will be available after native build.
        }
    }

    override fun onCreate() {
        super.onCreate()
        runningRef.set(false)
        AmbientNotificationHelper.createChannels(this)
        audioManager = getSystemService(Context.AUDIO_SERVICE) as? AudioManager
        telephonyManager = getSystemService(Context.TELEPHONY_SERVICE) as? TelephonyManager
    }

    override fun onStartCommand(
        intent: Intent?,
        flags: Int,
        startId: Int,
    ): Int {
        if (intent?.action == ACTION_STOP) {
            // User explicitly stopped listening via the notification or the
            // settings toggle. Disable re-arm so the service does not restart
            // in a loop.
            AmbientPreferences(this).setAmbientEnabled(false)
            stopSelf(startId)
            return START_NOT_STICKY
        }

        // Respect the user's persisted preference. System restarts or stale
        // start requests can arrive after the toggle has been turned off.
        if (!AmbientPreferences(this).isAmbientEnabled()) {
            stopSelf(startId)
            return START_NOT_STICKY
        }

        if (
            ContextCompat.checkSelfPermission(this, android.Manifest.permission.RECORD_AUDIO) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            updateState("録音の許可が必要です")
            AmbientPreferences(this).setAmbientEnabled(false)
            stopSelf(startId)
            return START_NOT_STICKY
        }

        // POST_NOTIFICATIONS is not a hard requirement for the foreground
        // service notification; log and continue if it is missing.
        if (
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(this, android.Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            Log.w(TAG, "POST_NOTIFICATIONS not granted; continuing")
        }

        val notification = AmbientNotificationHelper.createPersistentNotification(this, currentState)
        try {
            ServiceCompat.startForeground(
                this,
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
            )
        } catch (e: Throwable) {
            Log.e(TAG, "failed to start foreground", e)
            stopSelf(startId)
            return START_NOT_STICKY
        }

        if (runningRef.get()) {
            return START_STICKY
        }

        if (
            lifecycleLock.tryLock(LIFECYCLE_LOCK_TIMEOUT_MS, TimeUnit.MILLISECONDS)
        ) {
            try {
                if (runningRef.get()) {
                    return START_STICKY
                }
                runningRef.set(true)
                AmbientPreferences(this).setAmbientEnabled(true)
                Thread { startAmbientPipeline() }.start()
            } finally {
                lifecycleLock.unlock()
            }
        } else {
            Log.w(TAG, "could not acquire lifecycle lock in onStartCommand")
            stopSelf(startId)
            return START_NOT_STICKY
        }

        return START_STICKY
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onDestroy() {
        lifecycleLock.lock()
        try {
            runningRef.set(false)

            releaseRecorder()

            val ambient = mobileAmbient
            mobileAmbient = null
            // Hand the ambient pipeline to a background thread: `join(5000)`
            // blocks for up to five seconds while the tokio task drains, which
            // must not happen on the main thread during onDestroy (ANR risk).
            if (ambient != null) {
                Thread {
                    try {
                        ambient.stop()
                    } catch (_: Exception) {
                        // Ignore stop failures.
                    }
                    try {
                        ambient.join(5000)
                    } catch (_: Exception) {
                        // Ignore join timeout/failure.
                    }
                    try {
                        ambient.destroy()
                    } catch (_: Exception) {
                        // Ignore destroy failures.
                    }
                }.start()
            }
        } finally {
            lifecycleLock.unlock()
        }

        abandonAudioFocus()
        unregisterPhoneStateListener()

        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)

        if (AmbientPreferences(this).isAmbientEnabled()) {
            AmbientNotificationHelper.postReArmNotification(this)
        }

        super.onDestroy()
    }

    private fun startAmbientPipeline() {
        try {
            if (!runningRef.get()) {
                mainHandler.post { stopSelf() }
                return
            }

            val prefs = AmbientPreferences(this)
            val modelDir =
                prefs.getModelDir().ifEmpty {
                    File(noBackupFilesDir, "takusu/models").absolutePath
                }
            val asrModel =
                prefs.getAsrModel().ifEmpty {
                    AmbientPreferences.DEFAULT_ASR_MODEL
                }
            val language =
                prefs.getLanguage().ifEmpty {
                    AmbientPreferences.DEFAULT_LANGUAGE
                }
            val wakeWordBackend =
                prefs.getWakeWordBackend().ifEmpty {
                    AmbientPreferences.DEFAULT_WAKE_WORD_BACKEND
                }

            val ambient =
                MobileAmbient(
                    modelDir,
                    asrModel,
                    language,
                    WAKE_WORD,
                    wakeWordBackend,
                    PRE_SPEECH_BUFFER_MS,
                    false,
                    ambientCallback,
                )

            if (!runningRef.get()) {
                try {
                    ambient.destroy()
                } catch (_: Exception) {
                    // Ignore.
                }
                mainHandler.post { stopSelf() }
                return
            }

            // Publish to the service only if it is still alive; otherwise destroy
            // here. Without the check, `onDestroy` could race ahead of this
            // publish, read null, and leave a fully started `MobileAmbient`
            // (tokio runtime, loaded models) alive for the process lifetime.
            val published =
                withLifecycleLock {
                    if (runningRef.get()) {
                        mobileAmbient = ambient
                        true
                    } else {
                        false
                    }
                }

            if (!published) {
                try {
                    ambient.destroy()
                } catch (_: Exception) {
                    // Ignore.
                }
                mainHandler.post { stopSelf() }
                return
            }

            ambient.start()

            if (!runningRef.get()) {
                return
            }

            setupAudioRecord()
            registerPhoneStateListener()
            updateState("待機中")
        } catch (e: Throwable) {
            Log.e(TAG, "failed to start ambient", e)
            updateState("起動失敗")
            hasError.set(true)
            AmbientPreferences(this).setAmbientEnabled(false)
            mainHandler.post { stopSelf() }
        }
    }

    private fun setupAudioRecord() {
        if (
            !lifecycleLock.tryLock(LIFECYCLE_LOCK_TIMEOUT_MS, TimeUnit.MILLISECONDS)
        ) {
            Log.w(TAG, "could not acquire lifecycle lock in setupAudioRecord")
            return
        }

        try {
            if (!runningRef.get() || paused.get()) {
                return
            }

            // Release any previous recorder before creating a new one.
            releaseRecorder()

            if (!runningRef.get() || paused.get()) {
                return
            }

            if (!MicrophoneCoordinator.tryAcquire()) {
                Log.e(TAG, "microphone is held by another component")
                hasError.set(true)
                updateState("マイク使用エラー")
                mainHandler.post { stopSelf() }
                return
            }

            try {
                val minBufferSize =
                    AudioRecord.getMinBufferSize(
                        SAMPLE_RATE,
                        AudioFormat.CHANNEL_IN_MONO,
                        AudioFormat.ENCODING_PCM_16BIT,
                    )
                check(minBufferSize > 0) { "microphone is unavailable" }

                val desiredSize = CHUNK_SIZE * 4
                val bufferSize = if (minBufferSize > desiredSize) minBufferSize else desiredSize

                val record =
                    AudioRecord(
                        MediaRecorder.AudioSource.VOICE_RECOGNITION,
                        SAMPLE_RATE,
                        AudioFormat.CHANNEL_IN_MONO,
                        AudioFormat.ENCODING_PCM_16BIT,
                        bufferSize,
                    )

                if (record.state != AudioRecord.STATE_INITIALIZED) {
                    record.release()
                    throw IllegalStateException("failed to initialize microphone")
                }

                val t = Thread { recordingLoop(record) }
                audioRecord = record
                recordingThread = t
                t.start()
            } catch (e: Throwable) {
                Log.e(TAG, "failed to initialize microphone", e)
                hasError.set(true)
                updateState("マイク初期化エラー")
                MicrophoneCoordinator.release()
                mainHandler.post { stopSelf() }
            }
        } finally {
            lifecycleLock.unlock()
        }
    }

    private fun recordingLoop(record: AudioRecord) {
        val buffer = ShortArray(CHUNK_SIZE)

        try {
            try {
                record.startRecording()
            } catch (e: IllegalStateException) {
                Log.e(TAG, "failed to start recording", e)
                hasError.set(true)
                updateState("録音起動エラー")
                return
            }

            try {
                while (runningRef.get()) {
                    if (paused.get()) {
                        // updatePaused() stops/releases the recorder and
                        // joins this thread; exit the loop once woken up.
                        break
                    }

                    val count = record.read(buffer, 0, buffer.size)

                    if (count > 0) {
                        val chunk = buffer.copyOf(count).asList()

                        try {
                            mobileAmbient?.feedPcmChunk(chunk)
                        } catch (e: Exception) {
                            Log.e(TAG, "feedPcmChunk failed", e)
                            hasError.set(true)
                            updateState("フィードエラー")
                            AmbientPreferences(this@TakusuAgentService).setAmbientEnabled(false)
                            break
                        }
                    } else if (count < 0) {
                        Log.e(TAG, "AudioRecord read error: $count")
                        break
                    }
                }
            } catch (e: Throwable) {
                Log.e(TAG, "recording loop failed", e)
                hasError.set(true)
                updateState("録音エラー")
            }
        } catch (e: Throwable) {
            Log.e(TAG, "recording loop setup failed", e)
            hasError.set(true)
            updateState("録音起動エラー")
        } finally {
            try {
                record.stop()
            } catch (_: Exception) {
                // Ignore.
            }
            record.release()
            MicrophoneCoordinator.release()

            if (runningRef.get() && !paused.get()) {
                runningRef.set(false)
                mainHandler.post { stopSelf() }
            }
        }
    }

    private fun updateState(text: String) {
        currentState = text
        mainHandler.post {
            if (runningRef.get()) {
                updateNotification()
            }
        }
    }

    private fun updateNotification() {
        val notification = AmbientNotificationHelper.createPersistentNotification(this, currentState)
        try {
            ServiceCompat.startForeground(
                this,
                NOTIFICATION_ID,
                notification,
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
            )
        } catch (e: Throwable) {
            Log.e(TAG, "failed to update foreground notification", e)
        }
    }

    private fun updatePaused() {
        val shouldPause = audioFocusPaused.get() || callPaused.get()
        val wasPaused = paused.getAndSet(shouldPause)

        if (shouldPause && !wasPaused) {
            if (
                !lifecycleLock.tryLock(LIFECYCLE_LOCK_TIMEOUT_MS, TimeUnit.MILLISECONDS)
            ) {
                Log.w(TAG, "could not acquire lifecycle lock to pause")
                return
            }

            try {
                releaseRecorder()

                updateState("待機中")
            } finally {
                lifecycleLock.unlock()
            }
        } else if (!shouldPause && wasPaused) {
            if (
                !lifecycleLock.tryLock(LIFECYCLE_LOCK_TIMEOUT_MS, TimeUnit.MILLISECONDS)
            ) {
                Log.w(TAG, "could not acquire lifecycle lock to resume")
                return
            }

            try {
                if (runningRef.get()) {
                    setupAudioRecord()
                    updateState("待機中")
                }
            } finally {
                lifecycleLock.unlock()
            }
        }
    }

    private fun requestAudioFocusForCommand() {
        val am = audioManager ?: return

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val request =
                AudioFocusRequest
                    .Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK)
                    .setAudioAttributes(
                        AudioAttributes
                            .Builder()
                            .setUsage(AudioAttributes.USAGE_ASSISTANT)
                            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                            .build(),
                    ).setOnAudioFocusChangeListener(audioFocusChangeListener)
                    .build()
            audioFocusRequest = request
            am.requestAudioFocus(request)
        } else {
            am.requestAudioFocus(
                audioFocusChangeListener,
                AudioManager.STREAM_MUSIC,
                AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK,
            )
        }
    }

    private fun abandonAudioFocus() {
        val am = audioManager ?: return

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            audioFocusRequest?.let { am.abandonAudioFocusRequest(it) }
        } else {
            am.abandonAudioFocus(audioFocusChangeListener)
        }
        audioFocusRequest = null
    }

    private fun registerPhoneStateListener() {
        val tm = telephonyManager ?: return

        if (
            ContextCompat.checkSelfPermission(this, android.Manifest.permission.READ_PHONE_STATE) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            Log.w(TAG, "READ_PHONE_STATE not granted; call-state monitoring disabled")
            return
        }

        try {
            registerPhoneStateListenerInternal(tm)
        } catch (e: SecurityException) {
            Log.w(TAG, "failed to register phone state listener", e)
        }
    }

    private fun registerPhoneStateListenerInternal(tm: TelephonyManager) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val callback =
                object : TelephonyCallback(), TelephonyCallback.CallStateListener {
                    override fun onCallStateChanged(state: Int) {
                        handleCallState(state)
                    }
                }
            telephonyCallback = callback

            val executor = ContextCompat.getMainExecutor(this)
            tm.registerTelephonyCallback(executor, callback)
        } else {
            val listener =
                object : PhoneStateListener() {
                    override fun onCallStateChanged(
                        state: Int,
                        phoneNumber: String?,
                    ) {
                        handleCallState(state)
                    }
                }
            phoneStateListener = listener
            tm.listen(listener, PhoneStateListener.LISTEN_CALL_STATE)
        }
    }

    private fun unregisterPhoneStateListener() {
        val tm = telephonyManager ?: return

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            telephonyCallback?.let { tm.unregisterTelephonyCallback(it) }
            telephonyCallback = null
        } else {
            phoneStateListener?.let { tm.listen(it, PhoneStateListener.LISTEN_NONE) }
            phoneStateListener = null
        }
    }

    private fun handleCallState(state: Int) {
        when (state) {
            TelephonyManager.CALL_STATE_IDLE -> {
                callPaused.set(false)
                updatePaused()
            }

            else -> {
                callPaused.set(true)
                updatePaused()
            }
        }
    }

    private fun <T> withLifecycleLock(action: () -> T): T {
        lifecycleLock.lock()
        try {
            return action()
        } finally {
            lifecycleLock.unlock()
        }
    }

    /**
     * Stop and clean up the recording thread and AudioRecord without double
     * releasing. The recording thread owns the AudioRecord: its `finally`
     * always stops and releases it once it leaves `record.read()`. Any
     * `release()` here would race that and could free the same native handle
     * twice if `join(1000)` times out. So we only call `stop()` to unblock
     * `read()` when the thread is still alive, join it, and independently free
     * the shared microphone flag (idempotent).
     */
    private fun releaseRecorder() {
        val record = audioRecord
        val thread = recordingThread
        audioRecord = null
        recordingThread = null

        if (thread != null && thread.isAlive) {
            try {
                record?.stop()
            } catch (_: Exception) {
                // Ignore; the recorder may already be stopped.
            }
            try {
                thread.join(1000)
            } catch (_: Exception) {
                // Ignore join timeout/interrupt. The thread's finally still
                // releases the recorder when it exits.
            }
        }
        MicrophoneCoordinator.release()
    }

    companion object {
        private const val TAG = "TakusuAgentService"

        const val ACTION_START = "dev.satler.takusu.START_AMBIENT"
        const val ACTION_STOP = "dev.satler.takusu.STOP_AMBIENT"

        private const val NOTIFICATION_ID = 0x7382_0001
        private const val SAMPLE_RATE = 16_000
        private const val CHUNK_MS = 160
        private const val CHUNK_SIZE = (SAMPLE_RATE * CHUNK_MS) / 1000
        private const val WAKE_WORD = "たくす"
        private const val PRE_SPEECH_BUFFER_MS = 800UL
        private const val LIFECYCLE_LOCK_TIMEOUT_MS = 1000L

        val isRunning: Boolean
            get() = runningRef.get()

        fun startIntent(context: Context): Intent =
            Intent(context, TakusuAgentService::class.java).apply {
                action = ACTION_START
            }

        fun stopIntent(context: Context): Intent =
            Intent(context, TakusuAgentService::class.java).apply {
                action = ACTION_STOP
            }
    }
}
