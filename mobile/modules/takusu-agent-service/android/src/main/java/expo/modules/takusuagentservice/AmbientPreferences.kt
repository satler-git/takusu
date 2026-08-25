package expo.modules.takusuagentservice

import android.content.Context
import android.content.SharedPreferences
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.util.Log
import java.security.KeyStore
import java.util.concurrent.atomic.AtomicBoolean
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

private const val PREFS_NAME = "takusu_ambient_prefs"
private const val KEYSTORE_ALIAS = "takusu_ambient_master_key"
private const val GCM_TAG_LENGTH = 128
private const val ENCRYPTED_PREFIX = "enc:"

class AmbientPreferences(
    context: Context,
) {
    private val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    init {
        maybeMigrate()
    }

    fun isAmbientEnabled(): Boolean = prefs.getBoolean(KEY_AMBIENT_ENABLED, false)

    fun setAmbientEnabled(enabled: Boolean) {
        prefs.edit().putBoolean(KEY_AMBIENT_ENABLED, enabled).apply()
    }

    fun getModelDir(): String = getEncryptedString(KEY_MODEL_DIR, "")

    fun setModelDir(modelDir: String) {
        setEncryptedString(KEY_MODEL_DIR, modelDir)
    }

    fun getAsrModel(): String = prefs.getString(KEY_ASR_MODEL, DEFAULT_ASR_MODEL) ?: DEFAULT_ASR_MODEL

    fun setAsrModel(asrModel: String) {
        prefs.edit().putString(KEY_ASR_MODEL, asrModel).apply()
    }

    fun getLanguage(): String = prefs.getString(KEY_LANGUAGE, DEFAULT_LANGUAGE) ?: DEFAULT_LANGUAGE

    fun setLanguage(language: String) {
        prefs.edit().putString(KEY_LANGUAGE, language).apply()
    }

    fun getWakeWordBackend(): String =
        prefs.getString(KEY_WAKE_WORD_BACKEND, DEFAULT_WAKE_WORD_BACKEND) ?: DEFAULT_WAKE_WORD_BACKEND

    fun setWakeWordBackend(wakeWordBackend: String) {
        prefs.edit().putString(KEY_WAKE_WORD_BACKEND, wakeWordBackend).apply()
    }

    fun getWorkersUrl(): String = getEncryptedString(KEY_WORKERS_URL, "")

    fun setWorkersUrl(workersUrl: String) {
        setEncryptedString(KEY_WORKERS_URL, workersUrl)
    }

    fun getRootToken(): String = getEncryptedString(KEY_ROOT_TOKEN, "")

    fun setRootToken(rootToken: String) {
        setEncryptedString(KEY_ROOT_TOKEN, rootToken)
    }

    fun getDeviceId(): String = getEncryptedString(KEY_DEVICE_ID, DEFAULT_DEVICE_ID)

    fun setDeviceId(deviceId: String) {
        setEncryptedString(KEY_DEVICE_ID, deviceId)
    }

    fun getLocalUrl(): String = getEncryptedString(KEY_LOCAL_URL, "")

    fun setLocalUrl(localUrl: String) {
        setEncryptedString(KEY_LOCAL_URL, localUrl)
    }

    fun setStartOptions(
        workersUrl: String,
        rootToken: String,
        deviceId: String,
        localUrl: String,
        modelDir: String,
        asrModel: String,
        language: String,
        wakeWordBackend: String,
    ) {
        prefs
            .edit()
            .putString(KEY_WORKERS_URL, encryptIfPossible(workersUrl))
            .putString(KEY_ROOT_TOKEN, encryptIfPossible(rootToken))
            .putString(KEY_DEVICE_ID, encryptIfPossible(deviceId))
            .putString(KEY_LOCAL_URL, encryptIfPossible(localUrl))
            .putString(KEY_MODEL_DIR, encryptIfPossible(modelDir))
            .putString(KEY_ASR_MODEL, asrModel)
            .putString(KEY_LANGUAGE, language)
            .putString(KEY_WAKE_WORD_BACKEND, wakeWordBackend)
            .apply()
    }

    private fun getEncryptedString(
        key: String,
        defaultValue: String,
    ): String {
        val stored = prefs.getString(key, defaultValue) ?: defaultValue
        return decryptIfNeeded(stored, defaultValue)
    }

    private fun setEncryptedString(
        key: String,
        value: String,
    ) {
        prefs.edit().putString(key, encryptIfPossible(value)).apply()
    }

    private fun encryptIfPossible(value: String): String {
        if (value.isEmpty()) {
            return value
        }
        return try {
            val key = getOrCreateAesKey() ?: return value
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, key)
            val iv = cipher.iv
            val encrypted = cipher.doFinal(value.toByteArray(Charsets.UTF_8))
            ENCRYPTED_PREFIX +
                base64Encode(iv) + ":" + base64Encode(encrypted)
        } catch (e: Exception) {
            Log.w("AmbientPreferences", "failed to encrypt value for $PREFS_NAME", e)
            value
        }
    }

    private fun decryptIfNeeded(
        stored: String,
        defaultValue: String,
    ): String {
        if (!stored.startsWith(ENCRYPTED_PREFIX)) {
            return stored
        }
        val payload = stored.removePrefix(ENCRYPTED_PREFIX)
        val parts = payload.split(":", limit = 2)
        if (parts.size != 2) {
            return defaultValue
        }
        return try {
            val key = getOrCreateAesKey() ?: return defaultValue
            val iv = base64Decode(parts[0])
            val encrypted = base64Decode(parts[1])
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(GCM_TAG_LENGTH, iv))
            String(cipher.doFinal(encrypted), Charsets.UTF_8)
        } catch (e: Exception) {
            Log.w("AmbientPreferences", "failed to decrypt value for $PREFS_NAME", e)
            defaultValue
        }
    }

    private fun maybeMigrate() {
        if (migrationDone.get()) {
            return
        }
        cachedAesKey = getOrCreateAesKey()
        if (cachedAesKey != null) {
            migratePlainValuesToEncrypted(prefs)
        }
        migrationDone.set(true)
    }

    private fun migratePlainValuesToEncrypted(prefs: SharedPreferences) {
        val sensitiveKeys =
            listOf(
                KEY_WORKERS_URL,
                KEY_ROOT_TOKEN,
                KEY_DEVICE_ID,
                KEY_LOCAL_URL,
                KEY_MODEL_DIR,
            )
        val edit = prefs.edit()
        var needsApply = false

        for (key in sensitiveKeys) {
            val value = prefs.getString(key, null) ?: continue
            if (value.isEmpty() || value.startsWith(ENCRYPTED_PREFIX)) {
                continue
            }
            edit.putString(key, encryptIfPossible(value))
            needsApply = true
        }

        if (needsApply) {
            edit.apply()
        }
    }

    companion object {
        private const val KEY_AMBIENT_ENABLED = "ambient_enabled"
        private const val KEY_MODEL_DIR = "model_dir"
        private const val KEY_ASR_MODEL = "asr_model"
        private const val KEY_LANGUAGE = "language"
        private const val KEY_WAKE_WORD_BACKEND = "wake_word_backend"
        private const val KEY_WORKERS_URL = "workers_url"
        private const val KEY_ROOT_TOKEN = "root_token"
        private const val KEY_DEVICE_ID = "device_id"
        private const val KEY_LOCAL_URL = "local_url"

        const val DEFAULT_ASR_MODEL = "sherpa-sense-voice-int8"
        const val DEFAULT_LANGUAGE = "ja"
        const val DEFAULT_DEVICE_ID = "mobile"
        const val DEFAULT_WAKE_WORD_BACKEND = "sherpa_kws"

        @Volatile
        private var cachedAesKey: SecretKey? = null

        private val migrationDone = AtomicBoolean(false)

        private fun base64Encode(input: ByteArray): String = Base64.encodeToString(input, Base64.NO_WRAP)

        private fun base64Decode(input: String): ByteArray = Base64.decode(input, Base64.NO_WRAP)

        private fun getOrCreateAesKey(): SecretKey? {
            cachedAesKey?.let { return it }
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) {
                return null
            }
            val key =
                try {
                    val keystore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
                    keystore.getKey(KEYSTORE_ALIAS, null) as? SecretKey
                        ?: generateAesKey()
                } catch (e: Exception) {
                    Log.w("AmbientPreferences", "failed to access Android Keystore", e)
                    null
                }
            cachedAesKey = key
            return key
        }

        private fun generateAesKey(): SecretKey? =
            try {
                val spec =
                    KeyGenParameterSpec
                        .Builder(
                            KEYSTORE_ALIAS,
                            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                        ).setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .setRandomizedEncryptionRequired(true)
                        .build()

                KeyGenerator
                    .getInstance("AES", "AndroidKeyStore")
                    .apply { init(spec) }
                    .generateKey()
            } catch (e: Exception) {
                Log.w("AmbientPreferences", "failed to generate AES key in Keystore", e)
                null
            }
    }
}
