package com.digitaltracer.switchboard.core

import java.math.BigInteger
import java.security.AlgorithmParameters
import java.security.KeyFactory
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.ECParameterSpec
import java.security.spec.ECPoint
import java.security.spec.ECPrivateKeySpec
import java.security.spec.ECPublicKeySpec
import java.security.PrivateKey
import java.security.PublicKey

/**
 * A device has two P-256 keys: `auth` signs token challenges without asking, `approval` signs
 * approvals and needs a fingerprint or the screen lock for every use.
 */
enum class KeyRole(val alias: String) {
    Auth("auth"),
    Approval("approval"),
}

interface DeviceKeys {
    /** SEC1 uncompressed (X9.63) public key. */
    fun publicKey(role: KeyRole): ByteArray

    /** DER ECDSA over SHA-256 of `message`. For `approval` the system asks for the user first. */
    suspend fun sign(message: String, role: KeyRole, reason: String): ByteArray

    /** Forgets both keys, e.g. when re-pairing. */
    fun reset()
}

/** In-memory software keys, for tests. */
class SoftwareKeys : DeviceKeys {
    private val keys = mutableMapOf<KeyRole, KeyPair>()

    @Synchronized
    private fun key(role: KeyRole): KeyPair = keys.getOrPut(role) {
        KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()
    }

    override fun publicKey(role: KeyRole): ByteArray = P256.sec1(key(role).public as ECPublicKey)

    override suspend fun sign(message: String, role: KeyRole, reason: String): ByteArray =
        P256.sign(key(role).private, message.toByteArray())

    @Synchronized
    override fun reset() = keys.clear()
}

/** P-256 in the encodings the hub uses: SEC1 uncompressed public keys and DER signatures. */
object P256 {
    val params: ECParameterSpec = AlgorithmParameters.getInstance("EC")
        .apply { init(ECGenParameterSpec("secp256r1")) }
        .getParameterSpec(ECParameterSpec::class.java)

    fun sec1(key: ECPublicKey): ByteArray =
        byteArrayOf(4) + key.w.affineX.toFixed32() + key.w.affineY.toFixed32()

    /** Null unless `bytes` is an uncompressed point on the curve. */
    fun publicKey(bytes: ByteArray): PublicKey? {
        if (bytes.size != 65 || bytes[0] != 4.toByte()) return null
        val x = BigInteger(1, bytes.copyOfRange(1, 33))
        val y = BigInteger(1, bytes.copyOfRange(33, 65))
        val p = (params.curve.field as java.security.spec.ECFieldFp).p
        if (x >= p || y >= p) return null
        val rhs = x.pow(3).add(params.curve.a.multiply(x)).add(params.curve.b).mod(p)
        if (y.multiply(y).mod(p) != rhs) return null
        return KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(ECPoint(x, y), params))
    }

    /** A private key from its 32-byte scalar, as the shared test vectors give them. */
    fun privateKey(scalar: ByteArray): PrivateKey =
        KeyFactory.getInstance("EC").generatePrivate(ECPrivateKeySpec(BigInteger(1, scalar), params))

    fun sign(key: PrivateKey, data: ByteArray): ByteArray =
        Signature.getInstance("SHA256withECDSA").run {
            initSign(key)
            update(data)
            sign()
        }

    fun verify(key: PublicKey, data: ByteArray, der: ByteArray): Boolean = try {
        Signature.getInstance("SHA256withECDSA").run {
            initVerify(key)
            update(data)
            verify(der)
        }
    } catch (_: java.security.SignatureException) {
        false
    }

    private fun BigInteger.toFixed32(): ByteArray {
        val raw = toByteArray().let { if (it.size > 32) it.copyOfRange(it.size - 32, it.size) else it }
        return ByteArray(32 - raw.size) + raw
    }
}
