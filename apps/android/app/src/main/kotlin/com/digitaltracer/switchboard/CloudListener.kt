package com.digitaltracer.switchboard

import android.content.Context
import android.media.MediaRecorder
import com.digitaltracer.switchboard.core.Transcription
import java.io.File

/** Records a listen for upload in the pinned format. The caller has the microphone permission. */
class CloudRecorder(private val context: Context) {
    private var recorder: MediaRecorder? = null

    /** Throws if the microphone can't be had. */
    fun start(file: File) {
        val r = MediaRecorder(context)
        recorder = r
        r.setAudioSource(MediaRecorder.AudioSource.VOICE_RECOGNITION)
        r.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4)
        r.setAudioEncoder(MediaRecorder.AudioEncoder.AAC)
        r.setAudioChannels(Transcription.CHANNELS)
        r.setAudioSamplingRate(Transcription.SAMPLE_RATE)
        r.setAudioEncodingBitRate(Transcription.BIT_RATE)
        r.setOutputFile(file)
        r.prepare()
        r.start()
    }

    /** The loudest reading since the last call, 0 to 32767. */
    fun amplitude(): Int = runCatching { recorder?.maxAmplitude ?: 0 }.getOrDefault(0)

    /** True if the file is complete. MediaRecorder throws on a stop before any audio arrived. */
    fun stop(): Boolean {
        val r = recorder ?: return false
        recorder = null
        val complete = runCatching { r.stop() }.isSuccess
        r.release()
        return complete
    }
}
