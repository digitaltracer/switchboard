package com.digitaltracer.switchboard

import android.app.KeyguardManager
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricManager.Authenticators.BIOMETRIC_STRONG
import android.hardware.biometrics.BiometricManager.Authenticators.BIOMETRIC_WEAK
import android.hardware.biometrics.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.Bundle
import android.os.CancellationSignal
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.digitaltracer.switchboard.core.P256
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.spec.ECGenParameterSpec

/**
 * The Android milestone's Keystore spike: is an approval key that needs a fingerprint or the
 * screen lock for every signature enforced by the key itself? Results go to logcat as SBSpike.
 */
class KeystoreSpikeActivity : ComponentActivity() {
    private val lines = mutableStateListOf<String>()
    private val alias = "spike-approval"
    private val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(Modifier.fillMaxSize()) {
                    Column(Modifier.safeDrawingPadding().padding(16.dp).verticalScroll(rememberScrollState())) {
                        Button(::createKey) { Text("1. Create the approval key") }
                        Button(::signWithoutUser) { Text("2. Sign without asking") }
                        Button({ signWithUser() }) { Text("3. Sign with fingerprint or PIN") }
                        Button(::deleteKey) { Text("4. Delete the key") }
                        for (line in lines) Text(line, fontFamily = FontFamily.Monospace, fontSize = 12.sp)
                    }
                }
            }
        }
        report()
    }

    private fun log(s: String) {
        Log.i("SBSpike", s)
        lines += s
    }

    private fun report() {
        val bm = getSystemService(BiometricManager::class.java)
        log("device ${Build.MANUFACTURER} ${Build.MODEL}, Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})")
        log("screen lock set: ${getSystemService(KeyguardManager::class.java).isDeviceSecure}")
        log("strong biometric: ${status(bm.canAuthenticate(BIOMETRIC_STRONG))}")
        log("weak biometric: ${status(bm.canAuthenticate(BIOMETRIC_WEAK))}")
        log("strong or screen lock: ${status(bm.canAuthenticate(BIOMETRIC_STRONG or DEVICE_CREDENTIAL))}")
        log("strongbox feature: ${packageManager.hasSystemFeature("android.hardware.strongbox_keystore")}")
    }

    private fun status(code: Int) = when (code) {
        BiometricManager.BIOMETRIC_SUCCESS -> "available"
        BiometricManager.BIOMETRIC_ERROR_NONE_ENROLLED -> "none enrolled"
        BiometricManager.BIOMETRIC_ERROR_NO_HARDWARE -> "no hardware"
        BiometricManager.BIOMETRIC_ERROR_HW_UNAVAILABLE -> "hardware unavailable"
        BiometricManager.BIOMETRIC_ERROR_SECURITY_UPDATE_REQUIRED -> "security update required"
        else -> "code $code"
    }

    private fun spec(strongBox: Boolean) = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN)
        .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
        .setDigests(KeyProperties.DIGEST_SHA256)
        .setUserAuthenticationRequired(true)
        // 0 = every use; either a strong biometric or the screen lock, like iOS's .userPresence.
        .setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG or KeyProperties.AUTH_DEVICE_CREDENTIAL)
        .setInvalidatedByBiometricEnrollment(false)
        .setUnlockedDeviceRequired(true)
        .setIsStrongBoxBacked(strongBox)
        .build()

    private fun createKey() {
        store.deleteEntry(alias)
        val pair = try {
            KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore")
                .apply { initialize(spec(strongBox = true)) }.generateKeyPair().also { log("created in StrongBox") }
        } catch (e: StrongBoxUnavailableException) {
            log("no StrongBox (${e.javaClass.simpleName}); creating in the TEE")
            KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore")
                .apply { initialize(spec(strongBox = false)) }.generateKeyPair()
        } catch (e: Exception) {
            log("create failed: ${e.javaClass.simpleName}: ${e.message}")
            return
        }
        log("public key (SEC1) ${P256.sec1(pair.public as java.security.interfaces.ECPublicKey).size} bytes")
        val info = KeyFactory.getInstance(pair.private.algorithm, "AndroidKeyStore").getKeySpec(pair.private, KeyInfo::class.java)
        log("security level: ${level(info.securityLevel)}")
        log("auth required: ${info.isUserAuthenticationRequired}")
        log("auth enforced by secure hardware: ${info.isUserAuthenticationRequirementEnforcedBySecureHardware}")
        log("auth types: ${authTypes(info.userAuthenticationType)}")
        log("auth validity seconds: ${info.userAuthenticationValidityDurationSeconds} (0 = every use)")
        log("invalidated by new biometric: ${info.isInvalidatedByBiometricEnrollment}")
        log("chain from attestation: ${store.getCertificateChain(alias)?.size ?: 0} certs")
    }

    private fun level(l: Int) = when (l) {
        KeyProperties.SECURITY_LEVEL_STRONGBOX -> "StrongBox"
        KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT -> "TEE"
        KeyProperties.SECURITY_LEVEL_SOFTWARE -> "software"
        KeyProperties.SECURITY_LEVEL_UNKNOWN_SECURE -> "unknown secure"
        else -> "unknown ($l)"
    }

    private fun authTypes(t: Int) = listOfNotNull(
        "biometric strong".takeIf { t and KeyProperties.AUTH_BIOMETRIC_STRONG != 0 },
        "device credential".takeIf { t and KeyProperties.AUTH_DEVICE_CREDENTIAL != 0 },
    ).joinToString(" + ").ifEmpty { "none ($t)" }

    private fun key(): PrivateKey? = (store.getKey(alias, null) as PrivateKey?).also { if (it == null) log("no key; create it first") }

    private fun signWithoutUser() {
        val key = key() ?: return
        try {
            val sig = Signature.getInstance("SHA256withECDSA").apply { initSign(key) }
            sig.update("no user".toByteArray())
            sig.sign()
            log("FAIL: signed without the user")
        } catch (e: Exception) {
            log("refused without the user, as it should be: ${e.javaClass.simpleName}: ${e.message}")
        }
    }

    private fun signWithUser() {
        val key = key() ?: return
        val sig = try {
            Signature.getInstance("SHA256withECDSA").apply { initSign(key) }
        } catch (e: Exception) {
            log("initSign failed: ${e.javaClass.simpleName}: ${e.message}")
            return
        }
        BiometricPrompt.Builder(this)
            .setTitle("Approve in Switchboard")
            .setSubtitle("Keystore spike")
            .setAllowedAuthenticators(BIOMETRIC_STRONG or DEVICE_CREDENTIAL)
            .build()
            .authenticate(BiometricPrompt.CryptoObject(sig), CancellationSignal(), mainExecutor, object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    val how = when (result.authenticationType) {
                        BiometricPrompt.AUTHENTICATION_RESULT_TYPE_BIOMETRIC -> "biometric"
                        BiometricPrompt.AUTHENTICATION_RESULT_TYPE_DEVICE_CREDENTIAL -> "screen lock"
                        else -> "type ${result.authenticationType}"
                    }
                    val message = "approve a1".toByteArray()
                    try {
                        val s = result.cryptoObject!!.signature!!
                        s.update(message)
                        val der = s.sign()
                        val public = store.getCertificate(alias).publicKey
                        log("signed after $how; verifies: ${P256.verify(public, message, der)}")
                    } catch (e: Exception) {
                        log("FAIL: sign after $how threw ${e.javaClass.simpleName}: ${e.message}")
                        return
                    }
                    try {
                        val again = result.cryptoObject!!.signature!!
                        again.update(message)
                        again.sign()
                        log("FAIL: a second signature needed no new unlock")
                    } catch (e: Exception) {
                        log("second signature refused, as it should be: ${e.javaClass.simpleName}")
                    }
                }

                override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                    log("prompt error $errorCode: $errString")
                }
            })
    }

    private fun deleteKey() {
        store.deleteEntry(alias)
        log("deleted")
    }
}
