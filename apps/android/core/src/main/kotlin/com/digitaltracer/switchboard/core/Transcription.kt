package com.digitaltracer.switchboard.core

import java.io.File
import java.io.IOException
import java.io.InterruptedIOException
import java.net.ConnectException
import java.net.NoRouteToHostException
import java.net.SocketException
import java.net.UnknownHostException
import java.util.UUID
import java.util.concurrent.TimeUnit
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.async
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.Call
import okhttp3.Callback
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.MultipartBody
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.asRequestBody
import okhttp3.Response

/** Cloud voice: one recording format, one model, one host. */
object Transcription {
    const val MODEL = "gpt-transcribe"
    const val SAMPLE_RATE = 16_000
    const val CHANNELS = 1
    const val BIT_RATE = 32_000
    const val FILE_EXTENSION = "m4a"

    /** No response in this long counts as a failed upload. */
    const val UPLOAD_TIMEOUT_MS = 20_000L

    /** How long a failed recording may wait for Retry. */
    const val KEPT_FOR_MS = 600_000L
    const val MAX_NAMES = 100
    const val MAX_NAME_LENGTH = 100
}

enum class TranscriptionFailure(val message: String) {
    Offline("No connection reached OpenAI. Your recording is kept for now, so tap Retry."),
    Timeout("OpenAI didn't answer in time. Your recording is kept for now, so tap Retry."),
    Rate("OpenAI is limiting requests right now. Your recording is kept for now, so tap Retry."),
    Server("OpenAI couldn't transcribe that. Your recording is kept for now, so tap Retry."),
    Malformed("OpenAI's answer couldn't be read. Your recording is kept for now, so tap Retry."),
    Interrupted("The upload stopped when Switchboard left the screen. Your recording is kept for now, so tap Retry."),
    Auth("OpenAI refused this phone's key, so the recording was deleted. Voice stays on this phone until you replace the key in Settings."),
    Quota("This OpenAI key is out of quota, so the recording was deleted. Voice stays on this phone until you replace the key in Settings."),

    /** The recording can't be read, so there's nothing to send again. */
    Unreadable("The recording is gone. Say it again, or type instead."),
    ;

    /** A rejected key or an exhausted account: resending can't help until the key changes. */
    val rejectsKey: Boolean get() = this == Auth || this == Quota
}

class TranscriptionException(val failure: TranscriptionFailure) : Exception(failure.message)

/** The request's body and the answer's text. Pure, so the shape can be tested on its own. */
object TranscriptionWire {
    private val audioType = "audio/mp4".toMediaType()

    /** The API refuses a whole request for a keyword with `<`, `>` or a line break. */
    fun keywords(names: List<String>): List<String> = names.asSequence()
        .map { it.trim() }
        .filter { it.isNotEmpty() && it.length <= Transcription.MAX_NAME_LENGTH && it.none { c -> c in "<>\r\n" } }
        .distinct()
        .take(Transcription.MAX_NAMES)
        .toList()

    fun body(audio: File, names: List<String>): MultipartBody = MultipartBody.Builder("switchboard-${UUID.randomUUID()}")
        .setType(MultipartBody.FORM)
        .addFormDataPart("model", Transcription.MODEL)
        .addFormDataPart("response_format", "json")
        .apply { keywords(names).forEach { addFormDataPart("keywords[]", it) } }
        .addFormDataPart("file", "recording.${Transcription.FILE_EXTENSION}", audio.asRequestBody(audioType))
        .build()

    fun text(from: String): String = try {
        Json.parseToJsonElement(from).jsonObject["text"]?.jsonPrimitive?.contentOrNull?.trim()
    } catch (_: SerializationException) {
        null
    } catch (_: IllegalArgumentException) {
        null
    } ?: throw TranscriptionException(TranscriptionFailure.Malformed)

    /** A status other than 200, by what the owner can do about it. A redirect is never followed. */
    fun failure(status: Int, body: String): TranscriptionFailure = when (status) {
        401, 403 -> TranscriptionFailure.Auth
        429 -> if (errorCode(body) == "insufficient_quota") TranscriptionFailure.Quota else TranscriptionFailure.Rate
        else -> TranscriptionFailure.Server
    }

    fun failure(e: IOException): TranscriptionFailure = when (e) {
        is InterruptedIOException -> TranscriptionFailure.Timeout
        is UnknownHostException, is ConnectException, is NoRouteToHostException, is SocketException -> TranscriptionFailure.Offline
        else -> TranscriptionFailure.Server
    }

    private fun errorCode(body: String): String? = runCatching {
        Json.parseToJsonElement(body).jsonObject["error"]?.jsonObject?.get("code")?.jsonPrimitive?.contentOrNull
    }.getOrNull()
}

fun interface TranscriptionTransport {
    suspend fun transcribe(file: File, names: List<String>): String
}

/**
 * Uploads straight to OpenAI on its own client, with no redirects. It shares nothing with the
 * hub's client, and the app module can't point it anywhere else.
 */
class TranscriptionClient internal constructor(
    private val key: String,
    private val baseUrl: String,
    private val http: OkHttpClient,
) : TranscriptionTransport {
    constructor(key: String) : this(key, PROVIDER, shared)

    override suspend fun transcribe(file: File, names: List<String>): String {
        if (!file.canRead()) throw TranscriptionException(TranscriptionFailure.Unreadable)
        val request = try {
            Request.Builder()
                .url("$baseUrl/v1/audio/transcriptions")
                .header("Authorization", "Bearer $key")
                .post(TranscriptionWire.body(file, names))
                .build()
        } catch (_: IllegalArgumentException) {
            // A key no header can carry, which OpenAI would refuse too.
            throw TranscriptionException(TranscriptionFailure.Auth)
        }
        val (status, body) = execute(http.newCall(request))
        if (status != 200) throw TranscriptionException(TranscriptionWire.failure(status, body))
        return TranscriptionWire.text(body)
    }

    private suspend fun execute(call: Call): Pair<Int, String> = suspendCancellableCoroutine { c ->
        c.invokeOnCancellation { call.cancel() }
        call.enqueue(object : Callback {
            override fun onFailure(call: Call, e: IOException) {
                c.resumeWithException(TranscriptionException(TranscriptionWire.failure(e)))
            }

            override fun onResponse(call: Call, response: Response) {
                try {
                    val answer = response.use { it.code to it.body.string() }
                    c.resume(answer)
                } catch (e: IOException) {
                    c.resumeWithException(TranscriptionException(TranscriptionWire.failure(e)))
                }
            }
        })
    }

    internal companion object {
        private const val PROVIDER = "https://api.openai.com"
        private val shared by lazy { client(Transcription.UPLOAD_TIMEOUT_MS) }

        fun client(timeoutMs: Long): OkHttpClient = OkHttpClient.Builder()
            .followRedirects(false)
            .followSslRedirects(false)
            .callTimeout(timeoutMs, TimeUnit.MILLISECONDS)
            .build()
    }
}

/**
 * When a cloud listen ends, from loudness alone: batch transcription has no partial text to go by.
 * Levels are in dBFS. The floor follows the room, so a noisy room isn't taken for speech.
 */
class SpeechGate {
    sealed interface Decision {
        data object KeepGoing : Decision
        data class Stop(val heardSpeech: Boolean) : Decision
    }

    var heardSpeech = false
        private set
    private var startedAt: Long? = null
    private var lastAt: Long? = null
    private var floor: Float? = null
    private var loudSince: Long? = null
    private var lastSpeech = 0L

    fun add(level: Float, atMs: Long): Decision {
        val started = startedAt ?: atMs.also { startedAt = it }
        val elapsed = lastAt?.let { (atMs - it).coerceAtLeast(0) / 1000f } ?: 0f
        lastAt = atMs
        if (level > DIGITAL_SILENCE) {
            floor = floor?.let { minOf(level, it + FLOOR_RISE_PER_SECOND * elapsed) } ?: minOf(level, STARTING_FLOOR)
        }
        val room = floor ?: STARTING_FLOOR
        if (level > maxOf(room + SPEECH_ABOVE_FLOOR, QUIETEST_SPEECH)) {
            val since = loudSince ?: atMs.also { loudSince = it }
            if (!heardSpeech && atMs - since >= ONSET_MS) {
                heardSpeech = true
                lastSpeech = atMs
            }
        } else {
            loudSince = null
        }
        if (heardSpeech && level > maxOf(room + STILL_TALKING_ABOVE_FLOOR, QUIETEST_SPEECH)) lastSpeech = atMs

        return when {
            atMs - started >= CAP_MS -> Decision.Stop(heardSpeech)
            heardSpeech -> if (atMs - lastSpeech >= QUIET_AFTER_SPEECH_MS) Decision.Stop(true) else Decision.KeepGoing
            atMs - started >= NO_SPEECH_MS -> Decision.Stop(false)
            else -> Decision.KeepGoing
        }
    }

    companion object {
        const val QUIET_AFTER_SPEECH_MS = 1_800L
        const val NO_SPEECH_MS = 8_000L
        const val CAP_MS = 90_000L

        /** Speech must hold this long, so a knock doesn't count. */
        private const val ONSET_MS = 200L
        private const val SPEECH_ABOVE_FLOOR = 12f
        private const val STILL_TALKING_ABOVE_FLOOR = 6f
        private const val QUIETEST_SPEECH = -55f

        /** A floor this high or lower at the start lets someone who speaks at once be heard. */
        private const val STARTING_FLOOR = -45f
        private const val FLOOR_RISE_PER_SECOND = 1f

        /** Below this is no audio at all, such as a recorder's first reading. */
        private const val DIGITAL_SILENCE = -85f

        /** `MediaRecorder.getMaxAmplitude`, 0 to 32767, as dBFS. */
        fun decibels(amplitude: Int): Float =
            if (amplitude <= 0) -100f else (20 * kotlin.math.log10(amplitude / 32767f))

        /** For the level bars: about -50 dB is silence and -10 dB loud speech. */
        fun barLevel(level: Float): Float = ((level + 50f) / 40f).coerceIn(0f, 1f)
    }
}

/** Where recordings live: the app's temporary storage, which is never backed up. */
class RecordingStore(private val directory: File) {
    fun newFile(): File {
        directory.mkdirs()
        return File(directory, "${UUID.randomUUID()}.${Transcription.FILE_EXTENSION}")
    }

    /** Clears whatever a crash left behind. */
    fun sweep() {
        directory.deleteRecursively()
    }
}

/**
 * One recording from upload to deletion. A failed upload keeps the file for Retry, for at most
 * [Transcription.KEPT_FOR_MS]. Anything else that ends it deletes the file, and a later call makes
 * an earlier upload's answer count for nothing. Called from one thread.
 */
class RecordingLifecycle(
    private val scope: CoroutineScope,
    private val now: () -> Long = System::currentTimeMillis,
    private val delete: (File) -> Unit = { it.delete() },
) {
    sealed interface Outcome {
        /** The file is deleted. The text may be empty. */
        data class Text(val text: String) : Outcome

        /** The file is kept for Retry. */
        data class Kept(val failure: TranscriptionFailure) : Outcome

        /** The file is deleted: resending can't help. */
        data class Lost(val failure: TranscriptionFailure) : Outcome

        /** A later call took over, so this answer is ignored. */
        data object Discarded : Outcome
        data object Expired : Outcome
    }

    private var file: File? = null
    private var transport: TranscriptionTransport? = null
    private var names: List<String> = emptyList()
    private var keptSince: Long? = null
    private var generation = 0
    private var inFlight: Deferred<Result<String>>? = null

    /** A recording is waiting for Retry, with no upload running. */
    val canRetry: Boolean get() = file != null && keptSince != null && inFlight == null
    val uploading: Boolean get() = inFlight != null
    val expiresAt: Long? get() = keptSince?.plus(Transcription.KEPT_FOR_MS)

    /** A new recording replaces any earlier one and uploads. */
    suspend fun start(file: File, transport: TranscriptionTransport, names: List<String>): Outcome {
        discard()
        this.file = file
        this.transport = transport
        this.names = names
        return send()
    }

    /** Sends the same recording again. A second tap supersedes the first. */
    suspend fun retry(): Outcome {
        if (expireIfDue()) return Outcome.Expired
        if (file == null) return Outcome.Expired
        return send()
    }

    /** The app is leaving the foreground: the upload stops and the file stays for Retry. */
    fun interrupt() {
        if (inFlight == null) return
        stopUpload()
        keptSince = keptSince ?: now()
    }

    /** Any exit: deletes the file and ignores whatever is still in flight. */
    fun discard() {
        stopUpload()
        file?.let(delete)
        file = null
        transport = null
        names = emptyList()
        keptSince = null
    }

    /** True if the kept recording has just passed its limit and was deleted. */
    fun expireIfDue(): Boolean {
        val due = expiresAt ?: return false
        if (now() < due) return false
        discard()
        return true
    }

    private fun stopUpload() {
        generation++
        inFlight?.cancel()
        inFlight = null
    }

    private suspend fun send(): Outcome {
        val file = file ?: return Outcome.Expired
        val transport = transport ?: return Outcome.Expired
        stopUpload()
        val mine = generation
        val names = names
        val task = scope.async {
            try {
                Result.success(transport.transcribe(file, names))
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Result.failure(e)
            }
        }
        inFlight = task
        val result = try {
            task.await()
        } catch (e: CancellationException) {
            currentCoroutineContext().ensureActive()
            null
        }
        if (mine != generation) return Outcome.Discarded
        inFlight = null
        val failure = when {
            result == null -> TranscriptionFailure.Interrupted
            result.isSuccess -> {
                discard()
                return Outcome.Text(result.getOrThrow())
            }
            else -> (result.exceptionOrNull() as? TranscriptionException)?.failure ?: TranscriptionFailure.Server
        }
        if (failure.rejectsKey || failure == TranscriptionFailure.Unreadable) {
            discard()
            return Outcome.Lost(failure)
        }
        keptSince = keptSince ?: now()
        return Outcome.Kept(failure)
    }
}
