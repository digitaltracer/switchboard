import AVFoundation
import SwitchboardKit

/// Records a listen for upload. Levels are in dBFS.
@MainActor
protocol CloudRecording: AnyObject {
    func start(to file: URL, level: @escaping @MainActor (Float) -> Void) async throws
    func stop()
}

/// Records in the pinned format, asking only for the microphone: nothing here recognises speech.
@MainActor
final class CloudRecorder: CloudRecording {
    private var recorder: AVAudioRecorder?
    private var meter: Task<Void, Never>?
    private var stopped = false

    func start(to file: URL, level: @escaping @MainActor (Float) -> Void) async throws {
        guard await AVAudioApplication.requestRecordPermission() else { throw VoiceError.micDenied }
        guard !stopped else { return }
        try activateAudioSession()
        let recorder = try AVAudioRecorder(url: file, settings: [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: Transcription.sampleRate,
            AVNumberOfChannelsKey: Transcription.channels,
            AVEncoderBitRateKey: Transcription.bitRate,
        ])
        recorder.isMeteringEnabled = true
        guard recorder.record() else { throw VoiceError.notRecording }
        self.recorder = recorder
        meter = Task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(100))
                guard !Task.isCancelled else { return }
                recorder.updateMeters()
                level(recorder.averagePower(forChannel: 0))
            }
        }
    }

    func stop() {
        stopped = true
        meter?.cancel()
        recorder?.stop()
        recorder = nil
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }
}

/// A short burst of speech then quiet, so the real gate ends the listen: `SB_FAKE_TRANSCRIBE`.
@MainActor
final class ScriptedRecorder: CloudRecording {
    private var task: Task<Void, Never>?

    func start(to file: URL, level: @escaping @MainActor (Float) -> Void) async throws {
        try Data("scripted".utf8).write(to: file)
        task = Task {
            var tick = 0
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(100))
                tick += 1
                level(tick < 10 ? -60 : tick < 25 ? -15 : -60)
            }
        }
    }

    func stop() {
        task?.cancel()
    }
}

/// Canned answers for UI tests, one per upload and the last repeating: `SB_FAKE_TRANSCRIBE` holds
/// them separated by `|`, each a text or `error:<kind>`. `SB_FAKE_TRANSCRIBE_MS` delays each one.
final class ScriptedTranscriber: TranscriptionTransport, @unchecked Sendable {
    private let lock = NSLock()
    private var answers: [Substring]
    private let delay: Int

    init(script: String) {
        answers = script.split(separator: "|")
        delay = Int(ProcessInfo.processInfo.environment["SB_FAKE_TRANSCRIBE_MS"] ?? "") ?? 300
    }

    func transcribe(file _: URL, names _: [String]) async throws -> String {
        let answer = lock.withLock { answers.count > 1 ? answers.removeFirst() : answers.first ?? "" }
        try await Task.sleep(for: .milliseconds(delay))
        guard answer.hasPrefix("error:") else { return String(answer) }
        switch answer.dropFirst("error:".count) {
        case "auth": throw TranscriptionError.auth
        case "quota": throw TranscriptionError.quota
        case "timeout": throw TranscriptionError.timeout
        case "offline": throw TranscriptionError.offline
        default: throw TranscriptionError.server
        }
    }
}
