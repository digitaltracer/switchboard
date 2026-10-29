package com.digitaltracer.switchboard.core

import java.security.MessageDigest
import java.util.Base64

/** The hub's canonical signed messages; `src/auth/device.rs` builds the same strings. */
object SignedMessage {
    const val VERSION = 1

    fun token(hub: String, device: String, nonce: String, expiresAt: Long): String = listOf(
        "switchboard-token-v$VERSION",
        "hub=$hub",
        "device=$device",
        "nonce=$nonce",
        "expires=$expiresAt",
    ).joinToString("\n")

    /** Only approvals are signed; a deny needs just the device token. */
    fun approval(hub: String, device: String, approval: String, nonce: String, expiresAt: Long): String = listOf(
        "switchboard-approval-v$VERSION",
        "hub=$hub",
        "device=$device",
        "approval=$approval",
        "decision=allow",
        "nonce=$nonce",
        "expires=$expiresAt",
    ).joinToString("\n")

    /** `switchboard-answer-v1`: one chosen option of a question, plan or screen dialog. */
    const val ANSWER_VERSION = 3

    /**
     * [target] is `approval:<id>` for a hook question or plan, else `agent:<machine>/<agent>`;
     * [label] is [labelSha256] of the chosen labels.
     */
    fun answer(
        hub: String, device: String, target: String, occurrence: String, option: String,
        label: String, request: String, nonce: String, expiresAt: Long,
    ): String = listOf(
        "switchboard-answer-v1",
        "hub=$hub",
        "device=$device",
        "target=$target",
        "occurrence=$occurrence",
        "option=$option",
        "label=$label",
        "request=$request",
        "nonce=$nonce",
        "expires=$expiresAt",
    ).joinToString("\n")

    /** SHA-256 hex of the chosen labels joined by newlines, as the hub shows them. */
    fun labelSha256(labels: List<String>): String = sha256(labels.joinToString("\n").toByteArray()).hex()
}

/**
 * What the phone shows and the admin's terminal must match: SHA-256 over the hub id and both
 * public keys, as six groups of four hex digits.
 */
fun pairingFingerprint(hubId: String, authKey: String, approvalKey: String): String =
    sha256("$hubId\n$authKey\n$approvalKey".toByteArray()).copyOf(12).hex().chunked(4).joinToString("-")

internal fun sha256(data: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").digest(data)

internal fun ByteArray.hex(): String = joinToString("") { "%02x".format(it) }

val ByteArray.base64Url: String get() = Base64.getUrlEncoder().withoutPadding().encodeToString(this)

/** Null when `s` isn't base64url, with or without padding. */
fun base64UrlDecode(s: String): ByteArray? = try {
    Base64.getUrlDecoder().decode(s)
} catch (_: IllegalArgumentException) {
    null
}
