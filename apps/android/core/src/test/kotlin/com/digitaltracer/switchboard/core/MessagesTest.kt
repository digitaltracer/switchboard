package com.digitaltracer.switchboard.core

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

class MessagesTest {
    @Test
    fun messagesMatchTheHubs() {
        // The same strings `src/auth/device_tests.rs` pins.
        assertEquals(
            "switchboard-approval-v1\nhub=h1\ndevice=d1\napproval=a1\ndecision=allow\nnonce=n1\nexpires=42",
            SignedMessage.approval("h1", "d1", "a1", "n1", 42),
        )
        assertEquals(
            "switchboard-token-v1\nhub=h1\ndevice=d1\nnonce=n1\nexpires=42",
            SignedMessage.token("h1", "d1", "n1", 42),
        )
        assertEquals("a553-4998-583d-047b-1245-3ce8", pairingFingerprint("h1", "k1", "k2"))
    }

    @Test
    fun aCryptoKitSignatureVerifiesWithTheSameEncodings() {
        // The hub verifies Android's signatures the way it verifies this one from the iPhone.
        val f = Fixtures.json("cryptokit-signature.json")
        val key = P256.publicKey(base64UrlDecode(f.str("public_key"))!!)!!
        val der = base64UrlDecode(f.str("signature"))!!
        assertTrue(P256.verify(key, f.str("message").toByteArray(), der))
        assertTrue(!P256.verify(key, (f.str("message") + "x").toByteArray(), der))
    }

    @Test
    fun softwareKeysSignWhatTheHubVerifies() = kotlinx.coroutines.test.runTest {
        val keys = SoftwareKeys()
        val sec1 = keys.publicKey(KeyRole.Approval)
        assertEquals(65, sec1.size)
        val der = keys.sign("m", KeyRole.Approval, "test")
        assertTrue(P256.verify(P256.publicKey(sec1)!!, "m".toByteArray(), der))
        assertTrue(!P256.verify(P256.publicKey(keys.publicKey(KeyRole.Auth))!!, "m".toByteArray(), der))
    }

    @Test
    fun pointsOffTheCurveAreRefused() {
        val good = SoftwareKeys().publicKey(KeyRole.Auth)
        val bad = good.copyOf().also { it[64] = (it[64] + 1).toByte() }
        assertNull(P256.publicKey(bad))
        assertNull(P256.publicKey(good.copyOf(33)))
    }

    @Test
    fun pairingLinksAreParsedStrictly() {
        val link = PairingLink.parse("switchboard://pair?url=https%3A%2F%2Fhub.example.com&code=c0de&hub=h1")
        assertEquals("https://hub.example.com", link?.hubUrl)
        // The hub's own link ends its URL with a slash; requests must not start with `//`.
        val slashed = PairingLink.parse("switchboard://pair?url=http%3A%2F%2F127.0.0.1%3A8080%2F&code=c&hub=h")
        assertEquals("http://127.0.0.1:8080", slashed?.hubUrl)
        assertEquals("c0de", link?.code)
        assertEquals("h1", link?.hubId)
        assertNull(PairingLink.parse("https://pair?url=x&code=c&hub=h"))
        assertNull(PairingLink.parse("switchboard://pair?url=ftp%3A%2F%2Fx&code=c&hub=h"))
        assertNull(PairingLink.parse("switchboard://pair?url=https%3A%2F%2Fx&hub=h"))
    }

    @Test
    fun countdownsShowMinutesAndSeconds() {
        assertEquals("4:35", Countdown.text(10_000 + 275_000, 10_000))
        assertEquals("0:00", Countdown.text(1_000, 5_000))
        assertEquals("1:00", Countdown.text(61_999, 1_000))
    }
}
