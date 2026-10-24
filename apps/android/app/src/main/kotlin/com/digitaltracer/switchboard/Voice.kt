package com.digitaltracer.switchboard

import android.content.Context
import android.content.Intent
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
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Voice mode: listen, then review where the words go before they're sent. Speech is
 * transcribed on the phone; only the text ever reaches the hub.
 */
class Voice(private val context: Context, private val vocabulary: () -> List<String>) {
    enum class Phase { Idle, Listening, Review }

    private val scope = MainScope()
    private var recognizer: SpeechRecognizer? = null
    private var silence: Job? = null
    private var lastHeard = 0L

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

    fun listen(to: TargetView?, keepText: Boolean = false) {
        if (phase == Phase.Listening) return
        target = to
        if (!keepText) text = ""
        heard = ""
        error = null
        level = 0f
        phase = Phase.Listening
        changedAt = System.currentTimeMillis()
        lastHeard = changedAt
        if (!SpeechRecognizer.isOnDeviceRecognitionAvailable(context)) {
            error = UNAVAILABLE
            stop()
            return
        }
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
        release()
        text = listOf(text, heard).filter { it.isNotEmpty() }.joinToString(" ").trim()
        heard = ""
        level = 0f
        changedAt = System.currentTimeMillis()
        phase = if (text.isEmpty() && error == null) Phase.Idle else Phase.Review
    }

    /** Stops listening and opens the review with whatever was heard, even nothing. */
    fun typeInstead() {
        stop()
        phase = Phase.Review
    }

    fun type(to: TargetView?) {
        cancel()
        target = to
        phase = Phase.Review
    }

    fun cancel() {
        release()
        if (phase == Phase.Listening) changedAt = System.currentTimeMillis()
        phase = Phase.Idle
        text = ""
        heard = ""
        level = 0f
    }

    /** The review sheet was dismissed or its message went out. */
    fun finish() {
        phase = Phase.Idle
        text = ""
        error = null
    }

    fun denied() {
        error = "Switchboard needs the microphone. Allow it in Settings, or type instead."
        phase = Phase.Review
    }

    private fun release() {
        silence?.cancel()
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
            stop()
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
            stop()
        }

        override fun onReadyForSpeech(params: Bundle?) = Unit
        override fun onBeginningOfSpeech() = Unit
        override fun onBufferReceived(buffer: ByteArray?) = Unit
        override fun onEndOfSpeech() = Unit
        override fun onEvent(eventType: Int, params: Bundle?) = Unit
    }

    private companion object {
        const val UNAVAILABLE = "This phone can't transcribe your language on the device, so nothing was recorded. Type instead."
    }
}
