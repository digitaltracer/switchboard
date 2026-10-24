package com.digitaltracer.switchboard

import android.app.KeyguardManager
import android.content.Context
import android.hardware.biometrics.BiometricManager.Authenticators.BIOMETRIC_STRONG
import android.hardware.biometrics.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import android.hardware.biometrics.BiometricPrompt
import android.os.CancellationSignal
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import com.digitaltracer.switchboard.core.DeviceKeys
import com.digitaltracer.switchboard.core.KeyRole
import com.digitaltracer.switchboard.core.P256
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext

sealed class KeyError(message: String) : Exception(message) {
    class Cancelled : KeyError("Cancelled")
    class NoScreenLock : KeyError("Set a screen lock on this phone first: approvals need a fingerprint or the PIN.")
    class Invalidated : KeyError("This phone's approval key stopped working, e.g. because the screen lock was removed. Pair again.")
    class Failed(why: String) : KeyError(why)
}

/**
 * Keys in the Android Keystore, in StrongBox where the phone has one and the TEE otherwise. The
 * approval key needs a strong biometric or the screen lock for every signature, enforced by the
 * key itself; a new fingerprint doesn't invalidate it, like iOS's `.userPresence`.
 */
class KeystoreKeys(context: Context) : DeviceKeys {
    private val context = context.applicationContext
    private val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun alias(role: KeyRole) = "switchboard.${role.alias}"

    override fun publicKey(role: KeyRole): ByteArray {
        key(role)
        return P256.sec1(store.getCertificate(alias(role)).publicKey as ECPublicKey)
    }

    override suspend fun sign(message: String, role: KeyRole, reason: String): ByteArray {
        val data = message.toByteArray()
        val signature = withContext(Dispatchers.Default) {
            try {
                Signature.getInstance("SHA256withECDSA").apply { initSign(key(role)) }
            } catch (_: KeyPermanentlyInvalidatedException) {
                throw KeyError.Invalidated()
            }
        }
        if (role == KeyRole.Auth) {
            return withContext(Dispatchers.Default) { signature.run { update(data); sign() } }
        }
        val unlocked = withContext(Dispatchers.Main) { confirmUser(signature, reason) }
        return withContext(Dispatchers.Default) { unlocked.run { update(data); sign() } }
    }

    @Synchronized
    override fun reset() {
        for (role in KeyRole.entries) store.deleteEntry(alias(role))
    }

    @Synchronized
    private fun key(role: KeyRole): PrivateKey =
        store.getKey(alias(role), null) as PrivateKey? ?: generate(role)

    private fun generate(role: KeyRole): PrivateKey {
        if (role == KeyRole.Approval && !context.getSystemService(KeyguardManager::class.java).isDeviceSecure) {
            throw KeyError.NoScreenLock()
        }
        fun spec(strongBox: Boolean) = KeyGenParameterSpec.Builder(alias(role), KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .setIsStrongBoxBacked(strongBox)
            .apply {
                if (role == KeyRole.Approval) {
                    setUserAuthenticationRequired(true)
                    setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL)
                    setInvalidatedByBiometricEnrollment(false)
                    setUnlockedDeviceRequired(true)
                }
            }
            .build()
        val generator = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore")
        return try {
            generator.apply { initialize(spec(strongBox = true)) }.generateKeyPair().private
        } catch (_: StrongBoxUnavailableException) {
            generator.apply { initialize(spec(strongBox = false)) }.generateKeyPair().private
        }
    }

    /** Shows the system's fingerprint-or-screen-lock prompt, which unlocks one signature. */
    private suspend fun confirmUser(signature: Signature, reason: String): Signature = suspendCancellableCoroutine { cont ->
        val cancel = CancellationSignal()
        cont.invokeOnCancellation { cancel.cancel() }
        BiometricPrompt.Builder(context)
            .setTitle(reason)
            .setAllowedAuthenticators(BIOMETRIC_STRONG or DEVICE_CREDENTIAL)
            .build()
            .authenticate(BiometricPrompt.CryptoObject(signature), cancel, context.mainExecutor, object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    val unlocked = result.cryptoObject?.signature
                    if (unlocked == null) cont.resumeWithException(KeyError.Failed("The prompt returned no key.")) else cont.resume(unlocked)
                }

                override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                    val cancelled = errorCode == BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED ||
                        errorCode == BiometricPrompt.BIOMETRIC_ERROR_CANCELED
                    if (cont.isActive) cont.resumeWithException(if (cancelled) KeyError.Cancelled() else KeyError.Failed(errString.toString()))
                }
            })
    }
}
