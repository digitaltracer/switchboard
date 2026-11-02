package com.digitaltracer.switchboard.core

import java.io.File
import java.util.concurrent.TimeUnit
import kotlin.math.abs
import kotlin.test.AfterTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer

class TranscriptionClientTest {
    private val servers = mutableListOf<MockWebServer>()
    private val files = mutableListOf<File>()

    @AfterTest
    fun cleanUp() {
        servers.forEach { it.close() }
        files.forEach { it.delete() }
    }

    private fun server() = MockWebServer().also {
        it.start()
        servers += it
    }

    private fun recording() = File.createTempFile("rec", ".m4a").also {
        it.writeText("fake-audio")
        files += it
    }

    private fun client(server: MockWebServer, timeoutMs: Long = 5_000) =
        TranscriptionClient("sk-fake-test-key", server.url("/").toString().trimEnd('/'), TranscriptionClient.client(timeoutMs))

    private fun answer(status: Int, body: String) =
        MockResponse.Builder().code(status).addHeader("content-type", "application/json").body(body).build()

    @Test
    fun `the request carries the model the names and the audio with the key in the header`() = runTest {
        val s = server()
        s.enqueue(answer(200, """{"text":" run the benchmarks "}"""))

        assertEquals("run the benchmarks", client(s).transcribe(recording(), listOf("fix/retry-loop", "demo")))

        val request = s.takeRequest()
        assertEquals("POST", request.method)
        assertEquals("/v1/audio/transcriptions", request.url.encodedPath)
        assertEquals("Bearer sk-fake-test-key", request.headers["Authorization"])
        assertTrue(request.headers["Content-Type"]!!.startsWith("multipart/form-data; boundary=switchboard-"))
        val body = request.body!!.utf8()
        assertTrue(Regex("""name="model"\r\n(.*\r\n)*?\r\ngpt-transcribe\r\n""").containsMatchIn(body), body)
        assertTrue(Regex("""name="keywords\[]"\r\n(.*\r\n)*?\r\nfix/retry-loop\r\n""").containsMatchIn(body), body)
        assertTrue(Regex("""name="keywords\[]"\r\n(.*\r\n)*?\r\ndemo\r\n""").containsMatchIn(body), body)
        assertTrue(body.contains("""filename="recording.m4a""""), body)
        assertTrue(body.contains("audio/mp4"), body)
        assertTrue(body.contains("fake-audio"), body)
        assertFalse(body.contains("sk-fake-test-key"))
    }

    @Test
    fun `the names are bounded and what the API refuses is left out`() {
        assertEquals(Transcription.MAX_NAMES, TranscriptionWire.keywords((0 until 150).map { "name-$it" }).size)
        val long = "x".repeat(Transcription.MAX_NAME_LENGTH + 1)
        assertEquals(listOf("good", "padded"), TranscriptionWire.keywords(listOf("good", "a<b", "c>d", "e\nf", " ", long, "good", " padded ")))
    }

    private fun failureOf(status: Int, body: String) = runTest {
        val s = server()
        s.enqueue(answer(status, body))
        val e = assertFailsWith<TranscriptionException> { client(s).transcribe(recording(), emptyList()) }
        lastFailure = e.failure
    }

    private var lastFailure: TranscriptionFailure? = null

    @Test
    fun `each failure has its class`() {
        val cases = listOf(
            Triple(401, """{"error":{"code":"invalid_api_key"}}""", TranscriptionFailure.Auth),
            Triple(403, "{}", TranscriptionFailure.Auth),
            Triple(429, """{"error":{"code":"insufficient_quota"}}""", TranscriptionFailure.Quota),
            Triple(429, """{"error":{"code":"rate_limit_exceeded"}}""", TranscriptionFailure.Rate),
            Triple(500, "oops", TranscriptionFailure.Server),
            Triple(400, "{}", TranscriptionFailure.Server),
            Triple(200, "not json", TranscriptionFailure.Malformed),
            Triple(200, """{"other":1}""", TranscriptionFailure.Malformed),
        )
        for ((status, body, expected) in cases) {
            failureOf(status, body)
            assertEquals(expected, lastFailure, "$status $body")
        }
    }

    @Test
    fun `a redirect is not followed so the key goes nowhere else`() = runTest {
        val elsewhere = server()
        val s = server()
        s.enqueue(MockResponse.Builder().code(307).addHeader("Location", elsewhere.url("/v1/audio/transcriptions").toString()).build())

        val e = assertFailsWith<TranscriptionException> { client(s).transcribe(recording(), emptyList()) }

        assertEquals(TranscriptionFailure.Server, e.failure)
        assertEquals(1, s.requestCount)
        assertEquals(0, elsewhere.requestCount)
    }

    @Test
    fun `no response in time is a timeout`() = runTest {
        val s = server()
        s.enqueue(MockResponse.Builder().headersDelay(5, TimeUnit.SECONDS).body("{}").build())
        val e = assertFailsWith<TranscriptionException> { client(s, timeoutMs = 200).transcribe(recording(), emptyList()) }
        assertEquals(TranscriptionFailure.Timeout, e.failure)
    }

    @Test
    fun `no server is offline`() = runTest {
        val s = server()
        val closed = client(s)
        s.close()
        val e = assertFailsWith<TranscriptionException> { closed.transcribe(recording(), emptyList()) }
        assertEquals(TranscriptionFailure.Offline, e.failure)
    }

    @Test
    fun `a missing recording is unreadable and nothing is sent`() = runTest {
        val s = server()
        val e = assertFailsWith<TranscriptionException> { client(s).transcribe(File("/nonexistent/rec.m4a"), emptyList()) }
        assertEquals(TranscriptionFailure.Unreadable, e.failure)
        assertEquals(0, s.requestCount)
    }

    @Test
    fun `a key no header can carry counts as refused and stays out of the error`() = runTest {
        val s = server()
        val bad = TranscriptionClient("sk-fake\nkey", s.url("/").toString().trimEnd('/'), TranscriptionClient.client(1_000))
        val e = assertFailsWith<TranscriptionException> { bad.transcribe(recording(), emptyList()) }
        assertEquals(TranscriptionFailure.Auth, e.failure)
        assertFalse(e.message.orEmpty().contains("sk-fake"))
    }

    @Test
    fun `only a rejected key or quota stops cloud mode`() {
        assertEquals(setOf(TranscriptionFailure.Auth, TranscriptionFailure.Quota), TranscriptionFailure.entries.filter { it.rejectsKey }.toSet())
    }
}

class SpeechGateTest {
    /** Feeds a reading every 100 ms; returns the first stop and when it came. */
    private fun SpeechGate.run(level: Float, from: Long, to: Long): Pair<SpeechGate.Decision.Stop, Long>? {
        var t = from
        while (t < to) {
            val d = add(level, t)
            if (d is SpeechGate.Decision.Stop) return d to t
            t += 100
        }
        return null
    }

    @Test
    fun `speech then quiet stops after the pause`() {
        val gate = SpeechGate()
        assertNull(gate.run(-55f, 0, 1_000))
        assertNull(gate.run(-20f, 1_000, 4_000))
        val (stop, at) = assertNotNull(gate.run(-55f, 4_000, 8_000))
        assertEquals(SpeechGate.Decision.Stop(true), stop)
        assertTrue(abs(at - (3_900 + SpeechGate.QUIET_AFTER_SPEECH_MS)) <= 100, "$at")
    }

    @Test
    fun `silence stops at eight seconds with no speech`() {
        val (stop, at) = assertNotNull(SpeechGate().run(-60f, 0, 12_000))
        assertEquals(SpeechGate.Decision.Stop(false), stop)
        assertTrue(abs(at - SpeechGate.NO_SPEECH_MS) <= 100, "$at")
    }

    @Test
    fun `unbroken speech stops at the cap`() {
        val gate = SpeechGate()
        assertNull(gate.run(-60f, 0, 500))
        var t = 500L
        var stop: Pair<SpeechGate.Decision, Long>? = null
        while (t < 100_000 && stop == null) {
            // Syllables with short gaps, loud enough that the rising floor never reaches them.
            val d = gate.add(if ((t / 100) % 5 == 0L) -40f else -15f, t)
            if (d is SpeechGate.Decision.Stop) stop = d to t
            t += 100
        }
        val (decision, at) = assertNotNull(stop)
        assertEquals(SpeechGate.Decision.Stop(true), decision)
        assertTrue(abs(at - SpeechGate.CAP_MS) <= 100, "$at")
    }

    @Test
    fun `a steady noisy room is not speech but talking over it is`() {
        assertEquals(SpeechGate.Decision.Stop(false), assertNotNull(SpeechGate().run(-40f, 0, 12_000)).first)

        val talking = SpeechGate()
        assertNull(talking.run(-40f, 0, 2_000))
        assertNull(talking.run(-15f, 2_000, 4_000))
        assertTrue(talking.heardSpeech)
        assertEquals(SpeechGate.Decision.Stop(true), assertNotNull(talking.run(-40f, 4_000, 8_000)).first)
    }

    @Test
    fun `a knock is not speech`() {
        val gate = SpeechGate()
        assertNull(gate.run(-60f, 0, 2_000))
        assertEquals(SpeechGate.Decision.KeepGoing, gate.add(-10f, 2_000))
        assertEquals(SpeechGate.Decision.KeepGoing, gate.add(-60f, 2_100))
        assertFalse(gate.heardSpeech)
    }

    @Test
    fun `speaking from the first instant counts`() {
        val gate = SpeechGate()
        assertNull(gate.run(-20f, 0, 1_000))
        assertTrue(gate.heardSpeech)
    }

    @Test
    fun `a recorder's empty first reading does not set the floor`() {
        val gate = SpeechGate()
        assertEquals(SpeechGate.Decision.KeepGoing, gate.add(SpeechGate.decibels(0), 0))
        // The room is -40: with a floor of -100 it would pass as speech.
        assertEquals(SpeechGate.Decision.Stop(false), assertNotNull(gate.run(-40f, 100, 12_000)).first)
    }

    @Test
    fun `amplitude converts to decibels`() {
        assertEquals(0f, SpeechGate.decibels(32767), 0.01f)
        assertEquals(-20f, SpeechGate.decibels(3277), 0.05f)
        assertEquals(-100f, SpeechGate.decibels(0))
    }
}

/** With no answers given, each call waits for [answer]. */
private class FakeTransport(answers: List<Result<String>> = emptyList()) : TranscriptionTransport {
    private val answers = answers.toMutableList()
    private val waiting = mutableListOf<CompletableDeferred<String>>()
    val sentNames = mutableListOf<List<String>>()

    override suspend fun transcribe(file: File, names: List<String>): String {
        sentNames += names
        if (answers.isNotEmpty()) return answers.removeAt(0).getOrThrow()
        val d = CompletableDeferred<String>()
        waiting += d
        try {
            return d.await()
        } finally {
            waiting -= d
        }
    }

    fun answer(text: String) {
        waiting.first().complete(text)
    }
}

private fun failed(f: TranscriptionFailure) = Result.failure<String>(TranscriptionException(f))

@OptIn(ExperimentalCoroutinesApi::class)
class RecordingLifecycleTest {
    private val file = File("/tmp/rec.m4a")
    private val next = File("/tmp/next.m4a")
    private var time = 1_000_000L
    private val deleted = mutableListOf<File>()

    private fun TestScope.life() = RecordingLifecycle(backgroundScope, { time }, { deleted += it })

    @Test
    fun `success deletes the file`() = runTest {
        val life = life()
        assertEquals(RecordingLifecycle.Outcome.Text("hello"), life.start(file, FakeTransport(listOf(Result.success("hello"))), listOf("demo")))
        assertEquals(listOf(file), deleted)
        assertFalse(life.canRetry)
    }

    @Test
    fun `a failure keeps the file and retry sends the same one only when asked`() = runTest {
        val life = life()
        val fake = FakeTransport(listOf(failed(TranscriptionFailure.Server), Result.success("second time")))
        assertEquals(RecordingLifecycle.Outcome.Kept(TranscriptionFailure.Server), life.start(file, fake, listOf("demo")))
        runCurrent()
        assertTrue(deleted.isEmpty())
        assertTrue(life.canRetry)
        assertEquals(1, fake.sentNames.size, "nothing resends by itself")

        assertEquals(RecordingLifecycle.Outcome.Text("second time"), life.retry())
        assertEquals(listOf(listOf("demo"), listOf("demo")), fake.sentNames)
        assertEquals(listOf(file), deleted)
    }

    @Test
    fun `a rejected key deletes the file`() = runTest {
        for (failure in listOf(TranscriptionFailure.Auth, TranscriptionFailure.Quota, TranscriptionFailure.Unreadable)) {
            deleted.clear()
            val life = life()
            assertEquals(RecordingLifecycle.Outcome.Lost(failure), life.start(file, FakeTransport(listOf(failed(failure))), emptyList()))
            assertEquals(listOf(file), deleted)
            assertFalse(life.canRetry)
        }
    }

    @Test
    fun `a kept file expires after ten minutes even across retries`() = runTest {
        val life = life()
        val fake = FakeTransport(listOf(failed(TranscriptionFailure.Timeout), failed(TranscriptionFailure.Timeout)))
        life.start(file, fake, emptyList())
        assertEquals(time + 600_000, life.expiresAt)
        val limit = life.expiresAt

        time += 300_000
        assertFalse(life.expireIfDue())
        life.retry()
        assertEquals(limit, life.expiresAt, "a retry doesn't extend it")

        time += 300_000
        assertEquals(RecordingLifecycle.Outcome.Expired, life.retry())
        assertEquals(listOf(file), deleted)
        assertEquals(2, fake.sentNames.size, "an expired recording isn't sent")
    }

    @Test
    fun `the expiry timer deletes the file with no tap`() = runTest {
        val life = life()
        life.start(file, FakeTransport(listOf(failed(TranscriptionFailure.Server))), emptyList())
        time += 600_001
        assertTrue(life.expireIfDue())
        assertEquals(listOf(file), deleted)
        assertFalse(life.canRetry)
    }

    @Test
    fun `a sheet exit deletes a kept file once`() = runTest {
        val life = life()
        life.start(file, FakeTransport(listOf(failed(TranscriptionFailure.Server))), emptyList())
        life.discard()
        assertEquals(listOf(file), deleted)
        life.discard()
        assertEquals(listOf(file), deleted)
    }

    @Test
    fun `a new recording deletes the kept one`() = runTest {
        val life = life()
        life.start(file, FakeTransport(listOf(failed(TranscriptionFailure.Server))), emptyList())
        val outcome = life.start(next, FakeTransport(listOf(failed(TranscriptionFailure.Server))), emptyList())
        assertIs<RecordingLifecycle.Outcome.Kept>(outcome)
        assertEquals(listOf(file), deleted)
        assertTrue(life.canRetry)
    }

    @Test
    fun `a double retry ignores the first answer`() = runTest {
        val life = life()
        val fake = FakeTransport(listOf(failed(TranscriptionFailure.Server)))
        life.start(file, fake, emptyList())

        val one = async { life.retry() }
        runCurrent()
        val two = async { life.retry() }
        runCurrent()
        assertEquals(3, fake.sentNames.size)

        assertEquals(RecordingLifecycle.Outcome.Discarded, one.await())
        assertTrue(deleted.isEmpty(), "the second retry still needs the file")

        fake.answer("from the second retry")
        assertEquals(RecordingLifecycle.Outcome.Text("from the second retry"), two.await())
        assertEquals(listOf(file), deleted)
    }

    @Test
    fun `dismissing during a retry discards the answer`() = runTest {
        val life = life()
        val fake = FakeTransport(listOf(failed(TranscriptionFailure.Server)))
        life.start(file, fake, emptyList())

        val again = async { life.retry() }
        runCurrent()
        life.discard()
        assertEquals(RecordingLifecycle.Outcome.Discarded, again.await())
        assertEquals(listOf(file), deleted)
        assertFalse(life.canRetry)
        assertFalse(life.uploading)
    }

    @Test
    fun `a new listen during an upload discards the answer and keeps only the new file`() = runTest {
        val life = life()
        val first = async { life.start(file, FakeTransport(), emptyList()) }
        runCurrent()

        val new = FakeTransport()
        val second = async { life.start(next, new, emptyList()) }
        runCurrent()
        assertEquals(RecordingLifecycle.Outcome.Discarded, first.await())
        assertEquals(listOf(file), deleted)

        new.answer("new words")
        assertEquals(RecordingLifecycle.Outcome.Text("new words"), second.await())
        assertEquals(listOf(file, next), deleted)
    }

    @Test
    fun `going to the background stops the upload and keeps the file for retry`() = runTest {
        val life = life()
        val fake = FakeTransport()
        val upload = async { life.start(file, fake, emptyList()) }
        runCurrent()

        life.interrupt()
        assertTrue(life.canRetry)
        assertTrue(deleted.isEmpty())
        assertEquals(RecordingLifecycle.Outcome.Discarded, upload.await())

        val again = async { life.retry() }
        runCurrent()
        fake.answer("back again")
        assertEquals(RecordingLifecycle.Outcome.Text("back again"), again.await())
        assertEquals(listOf(file), deleted)
    }

    @Test
    fun `interrupting with no upload changes nothing`() = runTest {
        val life = life()
        life.interrupt()
        assertFalse(life.canRetry)
        assertNull(life.expiresAt)
    }

    @Test
    fun `the launch sweep clears what a crash left`() {
        val dir = File(System.getProperty("java.io.tmpdir"), "sweep-${System.nanoTime()}")
        val store = RecordingStore(dir)
        val left = store.newFile()
        assertEquals("m4a", left.extension)
        left.writeText("left behind")
        store.sweep()
        assertFalse(left.exists())
        store.sweep()
        assertEquals(dir, store.newFile().parentFile)
        store.sweep()
    }
}
