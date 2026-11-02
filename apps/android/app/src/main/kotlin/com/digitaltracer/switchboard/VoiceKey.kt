package com.digitaltracer.switchboard

import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * This phone's OpenAI key, apart from its signing keys: a Keystore AES-GCM key wraps it and only
 * the ciphertext goes in prefs. A key that can't be read means voice stays on the device.
 */
class VoiceKey(private val prefs: SharedPreferences) {
    var saved by mutableStateOf(read() != null)
        private set

    /** Told when the key is saved or removed, so voice can drop what depended on the old one. */
    var changed: ((removed: Boolean) -> Unit)? = null

    /** The key itself, which is never shown and never kept in this object. */
    fun read(): String? = runCatching {
        val sealed = Base64.decode(prefs.getString(PREF, null) ?: return null, Base64.NO_WRAP)
        val key = store().getKey(ALIAS, null) as SecretKey
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, sealed, 0, IV_BYTES))
        String(cipher.doFinal(sealed, IV_BYTES, sealed.size - IV_BYTES))
    }.getOrNull()

    /** False if the phone wouldn't store it, and then nothing is kept anywhere else. */
    fun save(value: String): Boolean {
        val trimmed = value.trim()
        if (trimmed.isEmpty()) return false
        val stored = runCatching {
            val cipher = Cipher.getInstance(TRANSFORM)
            cipher.init(Cipher.ENCRYPT_MODE, generate())
            val sealed = cipher.iv + cipher.doFinal(trimmed.toByteArray())
            prefs.edit().putString(PREF, Base64.encodeToString(sealed, Base64.NO_WRAP)).commit()
        }.getOrDefault(false)
        saved = read() != null
        if (stored) changed?.invoke(false)
        return stored
    }

    fun remove() {
        prefs.edit().remove(PREF).commit()
        runCatching { store().deleteEntry(ALIAS) }
        saved = false
        changed?.invoke(true)
    }

    private fun store() = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    /** Replaces any earlier key, so a removed token's wrapping key is gone too. */
    private fun generate(): SecretKey = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
        init(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        generateKey()
    }

    companion object {
        const val DISCLOSURE = "With a key saved, your voice recordings go to OpenAI to be transcribed, along with your project, branch, session and agent names so they come out spelled right. OpenAI bills this key. You still review the words before anything is sent. Without a key, speech is transcribed on this phone."

        private const val PREF = "voice_key_sealed"
        private const val ALIAS = "switchboard.voice-provider"
        private const val TRANSFORM = "AES/GCM/NoPadding"
        private const val IV_BYTES = 12
        private const val TAG_BITS = 128
    }
}
