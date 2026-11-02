package com.digitaltracer.switchboard

import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.Build
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.digitaltracer.switchboard.api.models.TargetView
import com.digitaltracer.switchboard.core.RecordingLifecycle
import com.digitaltracer.switchboard.core.RecordingStore
import com.digitaltracer.switchboard.core.SpeechGate
import com.digitaltracer.switchboard.core.TranscriptionClient
import com.digitaltracer.switchboard.core.TranscriptionFailure
import com.digitaltracer.switchboard.core.TranscriptionTransport
import java.io.File
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Voice mode: listen, then review where the words go before they're sent. Speech is
 * transcribed on the phone; only the text ever reaches the hub.
 */
class Voice(private val context: Context, private val key: VoiceKey, private val vocabulary: () -> List<String>) {
    enum class Phase { Idle, Listening, Transcribing, Review }

    /** What's running a listen on the device. */
    private interface Listener {
        fun start()
        fun stop()
    }

    private val scope = MainScope()
    private var listening: Listener? = null
    private var silence: Job? = null
    private var lastHeard = 0L

    private val store = RecordingStore(File(context.cacheDir, "voice-recordings"))
    private val recording = RecordingLifecycle(scope)

    /** OpenAI refused the key; cloud mode rests until it's replaced or the app relaunches. */
    private var keyRejected = false
    private var recorder: CloudRecorder? = null
    private var recordingFile: File? = null
    private var transport: TranscriptionTransport? = null
    private var meter: Job? = null
    private var expiry: Job? = null

    init {
        store.sweep()
        key.changed = { removed ->
            keyRejected = false
            // A listen under way would otherwise still upload with the removed key.
            if (removed) { if (cloud) cancel() else dropRecording() }
        }
    }

    var phase by mutableStateOf(Phase.Idle)
        private set

    /** Who the words are for; null lets the hub decide. */
    var target by mutableStateOf<TargetView?>(null)
        private set

    /** Text kept from before this listen, plus everything finished. */
    var text by mutableStateOf("")

    /** What the recognizer hears now, not yet part of `text`. */
    var heard by mutableStateOf("")
        private set

    /** Loudness, 0 to 1, smoothed. */
    var level by mutableFloatStateOf(0f)
        private set

    /** When listening last started or stopped, for the glow's sweep. */
    var changedAt by mutableLongStateOf(0L)
        private set

    var error by mutableStateOf<String?>(null)

    /** Why this listen stayed on the phone even though a key is saved. */
    var notice by mutableStateOf<String?>(null)
        private set

    /** This listen is recording for OpenAI. */
    var cloud by mutableStateOf(false)
        private set

    /** A failed recording can be sent again. */
    var canRetry by mutableStateOf(false)
        private set

    /** Send waits for a Retry to come back. */
    var retrying by mutableStateOf(false)
        private set

    fun listen(to: TargetView?, keepText: Boolean = false) {
        if (phase == Phase.Listening || phase == Phase.Transcribing) return
        dropRecording()
        notice = null
        target = to
        if (!keepText) text = ""
        heard = ""
        error = null
        level = 0f
        phase = Phase.Listening
        changedAt = System.currentTimeMillis()
        lastHeard = changedAt
        cloudTransport()?.let {
            startCloud(it)
            return
        }
        if (!SpeechRecognizer.isOnDeviceRecognitionAvailable(context)) {
            error = UNAVAILABLE
            stop()
            return
        }
        listening = OnDeviceListener().also { it.start() }
        // Like Siri, a pause after speaking ends the listen.
        silence = scope.launch {
            while (phase == Phase.Listening) {
                delay(250)
                val quiet = System.currentTimeMillis() - lastHeard
                if (heard.isNotEmpty() && quiet > 1_800 || heard.isEmpty() && quiet > 8_000) {
                    stop()
                    return@launch
                }
            }
        }
    }

    /** Ends the listen; what was heard goes to review, or nothing happens if nothing was. */
    fun stop() {
        if (phase != Phase.Listening) return
        if (cloud) {
            endCloudListen(upload = true)
            return
        }
        release()
        text = listOf(text, heard).filter { it.isNotEmpty() }.joinToString(" ").trim()
        heard = ""
        level = 0f
        changedAt = System.currentTimeMillis()
        phase = if (text.isEmpty() && error == null) Phase.Idle else Phase.Review
    }

    /** Stops listening and opens the review with whatever was heard, even nothing. */
    fun typeInstead() {
        if (cloud) endCloudListen(upload = false) else stop()
        phase = Phase.Review
    }

    fun type(to: TargetView?) {
        cancel()
        target = to
        phase = Phase.Review
    }

    fun cancel() {
        release()
        stopRecorder()
        recordingFile?.delete()
        recordingFile = null
        dropRecording()
        cloud = false
        notice = null
        if (phase == Phase.Listening) changedAt = System.currentTimeMillis()
        phase = Phase.Idle
        text = ""
        heard = ""
        level = 0f
    }

    /** The review sheet was dismissed or its message went out. */
    fun finish() {
        dropRecording()
        notice = null
        phase = Phase.Idle
        text = ""
        error = null
    }

    fun denied() {
        error = "Switchboard needs the microphone. Allow it in Settings, or type instead."
        phase = Phase.Review
    }

    /** Sends the kept recording again, only because the owner tapped Retry. */
    fun retry() {
        if (phase != Phase.Review || !(canRetry || retrying)) return
        error = null
        canRetry = false
        retrying = true
        scope.launch { settle(recording.retry()) }
    }

    /** An upload can't finish from the background, so it stops and waits for Retry. */
    fun wentToBackground() {
        if (phase != Phase.Transcribing && !retrying) return
        recording.interrupt()
        error = TranscriptionFailure.Interrupted.message
        retrying = false
        cloud = false
        watchExpiry()
        phase = Phase.Review
    }

    fun cameToForeground() {
        if (recording.expireIfDue()) expired()
    }

    private fun release() {
        silence?.cancel()
        listening?.stop()
        listening = null
    }

    // Cloud mode.

    /** Cloud with a usable key and a network path; anything else stays on the phone. */
    private fun cloudTransport(): TranscriptionTransport? {
        val saved = key.read() ?: return null
        if (keyRejected) {
            notice = "OpenAI won't accept this phone's key, so speech stays on this phone until you replace it."
            return null
        }
        if (!online()) {
            notice = "You're offline, so speech stays on this phone for now."
            return null
        }
        return TranscriptionClient(saved)
    }

    private fun online(): Boolean {
        val network = context.getSystemService(ConnectivityManager::class.java)
        return network.getNetworkCapabilities(network.activeNetwork)?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
    }

    private fun startCloud(transport: TranscriptionTransport) {
        cloud = true
        this.transport = transport
        val file = store.newFile()
        recordingFile = file
        val r = CloudRecorder(context)
        recorder = r
        try {
            r.start(file)
        } catch (_: Exception) {
            error = "Recording didn't start. Try again, or type instead."
            endCloudListen(upload = false)
            return
        }
        val gate = SpeechGate()
        meter = scope.launch {
            while (phase == Phase.Listening) {
                delay(100)
                val db = SpeechGate.decibels(r.amplitude())
                level = level * 0.6f + SpeechGate.barLevel(db) * 0.4f
                val decision = gate.add(db, System.currentTimeMillis())
                if (decision is SpeechGate.Decision.Stop) {
                    endCloudListen(upload = decision.heardSpeech)
                    return@launch
                }
            }
        }
    }

    /** Ends a cloud listen: uploads what was recorded, or deletes it and goes as an empty listen does. */
    private fun endCloudListen(upload: Boolean) {
        val transport = transport
        val file = recordingFile
        val complete = stopRecorder()
        recordingFile = null
        level = 0f
        changedAt = System.currentTimeMillis()
        if (!upload || !complete || file == null || transport == null) {
            file?.delete()
            cloud = false
            phase = if (text.isEmpty() && error == null) Phase.Idle else Phase.Review
            return
        }
        phase = Phase.Transcribing
        val names = vocabulary()
        scope.launch { settle(recording.start(file, transport, names)) }
    }

    /** Where an upload or a Retry came out. The review sheet shows the rest. */
    private fun settle(outcome: RecordingLifecycle.Outcome) {
        when (outcome) {
            RecordingLifecycle.Outcome.Discarded -> return
            is RecordingLifecycle.Outcome.Text -> {
                text = listOf(text, outcome.text).filter { it.isNotEmpty() }.joinToString(" ")
                error = null
            }
            is RecordingLifecycle.Outcome.Kept -> error = outcome.failure.message
            is RecordingLifecycle.Outcome.Lost -> {
                error = outcome.failure.message
                if (outcome.failure.rejectsKey) keyRejected = true
            }
            RecordingLifecycle.Outcome.Expired -> error = EXPIRED
        }
        retrying = false
        cloud = false
        watchExpiry()
        phase = if (text.isEmpty() && error == null) Phase.Idle else Phase.Review
    }

    private fun watchExpiry() {
        expiry?.cancel()
        canRetry = recording.canRetry
        val at = recording.expiresAt
        if (!canRetry || at == null) return
        expiry = scope.launch {
            delay((at - System.currentTimeMillis()).coerceAtLeast(0))
            if (recording.expireIfDue()) expired()
        }
    }

    private fun expired() {
        expiry?.cancel()
        canRetry = false
        retrying = false
        error = EXPIRED
    }

    /** True if the recorder finished a usable file. */
    private fun stopRecorder(): Boolean {
        meter?.cancel()
        val complete = recorder?.stop() ?: false
        recorder = null
        transport = null
        return complete
    }

    private fun dropRecording() {
        expiry?.cancel()
        recording.discard()
        canRetry = false
        retrying = false
    }

    /** The phone's own recognizer, held to the device. */
    private inner class OnDeviceListener : Listener {
        private var recognizer: SpeechRecognizer? = null

        override fun start() {
            val r = SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
            recognizer = r
            r.setRecognitionListener(listener)
            r.startListening(
                Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
                    putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
                    putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
                    putExtra(RecognizerIntent.EXTRA_PREFER_OFFLINE, true)
                    if (Build.VERSION.SDK_INT >= 33) {
                        putExtra(RecognizerIntent.EXTRA_ENABLE_FORMATTING, RecognizerIntent.FORMATTING_OPTIMIZE_QUALITY)
                        putStringArrayListExtra(RecognizerIntent.EXTRA_BIASING_STRINGS, ArrayList(vocabulary()))
                    }
                },
            )
        }

        override fun stop() {
            recognizer?.run {
                stopListening()
                destroy()
            }
            recognizer = null
        }

        private fun words(b: Bundle?) = b?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull().orEmpty()

        private val listener = object : RecognitionListener {
            override fun onPartialResults(partialResults: Bundle?) {
                if (phase != Phase.Listening) return
                val w = words(partialResults)
                if (w.isNotEmpty() && w != heard) {
                    heard = w
                    lastHeard = System.currentTimeMillis()
                }
            }

            override fun onResults(results: Bundle?) {
                if (phase != Phase.Listening) return
                words(results).takeIf { it.isNotEmpty() }?.let { heard = it }
                this@Voice.stop()
            }

            // About -2 dB is silence and 10 dB loud speech.
            override fun onRmsChanged(rmsdB: Float) {
                val l = ((rmsdB + 2f) / 12f).coerceIn(0f, 1f)
                level = level * 0.6f + l * 0.4f
            }

            override fun onError(code: Int) {
                if (phase != Phase.Listening) return
                when (code) {
                    SpeechRecognizer.ERROR_NO_MATCH, SpeechRecognizer.ERROR_SPEECH_TIMEOUT -> Unit
                    SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS -> error = "Switchboard needs the microphone. Allow it in Settings, or type instead."
                    SpeechRecognizer.ERROR_LANGUAGE_NOT_SUPPORTED, SpeechRecognizer.ERROR_LANGUAGE_UNAVAILABLE -> {
                        error = UNAVAILABLE
                        // The phone may be able to fetch its on-device model; type meanwhile.
                        if (Build.VERSION.SDK_INT >= 33) {
                            runCatching { recognizer?.triggerModelDownload(Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)) }
                        }
                    }
                    else -> error = "Listening stopped (speech error $code). Type instead, or try again."
                }
                this@Voice.stop()
            }

            override fun onReadyForSpeech(params: Bundle?) = Unit
            override fun onBeginningOfSpeech() = Unit
            override fun onBufferReceived(buffer: ByteArray?) = Unit
            override fun onEndOfSpeech() = Unit
            override fun onEvent(eventType: Int, params: Bundle?) = Unit
        }
    }

    private companion object {
        const val UNAVAILABLE = "This phone can't transcribe your language on the device, so nothing was recorded. Type instead."
        const val EXPIRED = "That recording was over ten minutes old, so it was deleted. Say it again, or type instead."
    }
}
