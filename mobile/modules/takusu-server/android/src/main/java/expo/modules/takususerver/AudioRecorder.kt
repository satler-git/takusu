package expo.modules.takususerver

import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import expo.modules.kotlin.exception.CodedException
import java.util.Collections
import java.util.concurrent.atomic.AtomicBoolean
import uniffi.takusu_android.MobileAudio

class AudioRecorder {
    private val running = AtomicBoolean(false)
    private val samples = Collections.synchronizedList(mutableListOf<Short>())
    private var recorder: AudioRecord? = null
    private var thread: Thread? = null
    private var started = false
    private var streaming = false
    private var audio: MobileAudio? = null
    private var language: String = ""
    private var transcript: String? = null
    private var error: Throwable? = null

    fun start() {
        startInternal(streamingAudio = null, language = "")
    }

    fun startStreaming(
        audio: MobileAudio,
        language: String,
    ) {
        startInternal(streamingAudio = audio, language = language)
    }

    private fun startInternal(
        streamingAudio: MobileAudio?,
        language: String,
    ) {
        synchronized(this) {
            check(!running.get()) { "recording is already running" }
            started = false
            streaming = streamingAudio != null
            audio = streamingAudio
            this.language = language
            transcript = null
            error = null

            streamingAudio?.startStreamingAsr(language)

            val minimumBuffer =
                AudioRecord.getMinBufferSize(
                    SAMPLE_RATE,
                    AudioFormat.CHANNEL_IN_MONO,
                    AudioFormat.ENCODING_PCM_16BIT,
                )
            check(minimumBuffer > 0) { "microphone is unavailable" }
            val audioRecord =
                AudioRecord(
                    MediaRecorder.AudioSource.VOICE_RECOGNITION,
                    SAMPLE_RATE,
                    AudioFormat.CHANNEL_IN_MONO,
                    AudioFormat.ENCODING_PCM_16BIT,
                    minimumBuffer * 2,
                )
            var success = false
            try {
                check(audioRecord.state == AudioRecord.STATE_INITIALIZED) { "failed to initialize microphone" }
                if (!streaming) {
                    samples.clear()
                }
                recorder = audioRecord
                val t =
                    Thread {
                        val buffer = ShortArray(minimumBuffer)
                        val chunk = mutableListOf<Short>()
                        var totalSamples = 0
                        try {
                            audioRecord.startRecording()
                            recording@
                            while (running.get() && totalSamples < SAMPLE_RATE * MAX_DURATION_SECONDS) {
                                val count = audioRecord.read(buffer, 0, buffer.size)
                                if (count > 0) {
                                    for (index in 0 until count) {
                                        if (error != null) {
                                            break@recording
                                        }
                                        val sample = buffer[index]
                                        if (streaming) {
                                            chunk.add(sample)
                                            if (chunk.size >= CHUNK_SIZE) {
                                                try {
                                                    audio!!.feedStreamingChunk(chunk.toList())
                                                } catch (e: Throwable) {
                                                    error = e
                                                    break@recording
                                                }
                                                chunk.clear()
                                            }
                                        } else {
                                            samples.add(sample)
                                        }
                                        totalSamples++
                                    }
                                }
                            }

                            if (streaming && error == null) {
                                if (chunk.isNotEmpty()) {
                                    try {
                                        audio!!.feedStreamingChunk(chunk)
                                    } catch (e: Throwable) {
                                        error = e
                                    }
                                }
                                if (error == null) {
                                    try {
                                        transcript = audio!!.finishStreamingAsr()
                                    } catch (e: Throwable) {
                                        error = e
                                    }
                                }
                            }
                        } finally {
                            try {
                                audioRecord.stop()
                            } catch (_: Exception) {
                                // Ignore; the recorder may already be stopped or
                                // was never started.
                            }
                            audioRecord.release()
                        }
                    }
                thread = t
                running.set(true)
                t.start()
                started = true
                success = true
            } finally {
                if (!success) {
                    running.set(false)
                    val t = thread
                    thread = null
                    recorder = null
                    if (t != null && t.isAlive) {
                        t.join()
                    } else {
                        try {
                            audioRecord.release()
                        } catch (_: Exception) {
                        }
                    }
                }
            }
        }
    }

    fun stop(): List<Short> {
        synchronized(this) {
            return if (running.compareAndSet(true, false)) {
                thread?.join()
                thread = null
                recorder = null
                started = false
                check(!streaming) { "called stop() on a streaming recorder" }
                synchronized(samples) { samples.toList() }
            } else {
                emptyList()
            }
        }
    }

    fun stopStreaming(): String {
        synchronized(this) {
            return if (running.compareAndSet(true, false)) {
                thread?.join()
                thread = null
                recorder = null
                started = false
                check(streaming) { "called stopStreaming() on a non-streaming recorder" }
                val cause = error
                if (cause != null) {
                    throw CodedException(
                        "ERR_STREAMING_ASR",
                        "Streaming ASR failed: ${cause.message}",
                        cause,
                    )
                }
                transcript ?: ""
            } else {
                ""
            }
        }
    }

    fun isStreaming(): Boolean = streaming

    companion object {
        const val SAMPLE_RATE = 16_000
        private const val MAX_DURATION_SECONDS = 60
        private const val CHUNK_MS = 160
        private val CHUNK_SIZE = (SAMPLE_RATE * CHUNK_MS) / 1000
    }
}
