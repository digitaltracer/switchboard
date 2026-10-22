import AVFoundation
import Observation
import Speech
import SwitchboardKit
import UIKit

/// Voice mode: listen, then review where the words go before they're sent. Speech is
/// transcribed on the phone; only the text ever reaches the hub.
@MainActor
@Observable
final class Voice {
    enum Phase: Equatable {
        case idle
        case listening
        /// The text is in the review sheet, typed or heard.
        case review
    }

    private(set) var phase: Phase = .idle
    /// Who the words are for; nil lets the hub decide.
    private(set) var context: Schemas.TargetView?
    /// Text kept from before this listen, plus everything finished.
    var text = ""
    /// What the recognizer hears now, not yet part of `text`.
    private(set) var heard = ""
    /// Loudness, 0 to 1, smoothed.
    private(set) var level: Float = 0
    /// When listening last started or stopped, for the glow's sweep.
    private(set) var changedAt = Date()
    private(set) var squeeze = false
    var error: String?

    private var listener: Listener?
    private var lastHeard = Date()
    private var silence: Task<Void, Never>?

    func listen(to context: Schemas.TargetView?, keepText: Bool = false) {
        guard phase != .listening else { return }
        self.context = context
        if !keepText { text = "" }
        heard = ""
        error = nil
        level = 0
        phase = .listening
        changedAt = Date()
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        squeeze = true
        Task { try? await Task.sleep(for: .milliseconds(170)); squeeze = false }
        let listener: Listener = ProcessInfo.processInfo.environment["SB_FAKE_VOICE"].map { ScriptedListener(text: $0) } ?? SpeechListener()
        self.listener = listener
        lastHeard = Date()
        Task {
            do {
                try await listener.start(
                    heard: { words in
                        guard self.phase == .listening else { return }
                        if words != self.heard { self.lastHeard = Date() }
                        self.heard = words
                    },
                    level: { l in
                        self.level = self.level * 0.6 + l * 0.4
                    }
                )
            } catch {
                guard phase == .listening else { return }
                self.error = error.localizedDescription
                stop()
            }
        }
        // Like Siri, a pause after speaking ends the listen.
        silence = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(250))
                guard let self, self.phase == .listening else { return }
                if !self.heard.isEmpty, Date().timeIntervalSince(self.lastHeard) > 1.8 { self.stop(); return }
                if self.heard.isEmpty, Date().timeIntervalSince(self.lastHeard) > 8 { self.stop(); return }
            }
        }
    }

    /// Ends the listen; what was heard goes to review, or nothing happens if nothing was.
    func stop() {
        guard phase == .listening else { return }
        silence?.cancel()
        listener?.stop()
        listener = nil
        let joined = [text, heard].filter { !$0.isEmpty }.joined(separator: " ")
        text = joined.trimmingCharacters(in: .whitespacesAndNewlines)
        heard = ""
        level = 0
        changedAt = Date()
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        phase = text.isEmpty && error == nil ? .idle : .review
    }

    /// Stops listening and opens the review with whatever was heard, even nothing.
    func typeInstead() {
        stop()
        phase = .review
    }

    func type(to context: Schemas.TargetView?) {
        cancel()
        self.context = context
        phase = .review
    }

    func cancel() {
        silence?.cancel()
        listener?.stop()
        listener = nil
        if phase == .listening { changedAt = Date() }
        phase = .idle
        text = ""
        heard = ""
        level = 0
    }

    /// The review sheet was dismissed or its message went out.
    func finish() {
        phase = .idle
        text = ""
        error = nil
    }
}

@MainActor
protocol Listener: AnyObject {
    func start(heard: @escaping @MainActor (String) -> Void, level: @escaping @MainActor (Float) -> Void) async throws
    func stop()
}

enum VoiceError: LocalizedError {
    case denied
    case unavailable

    var errorDescription: String? {
        switch self {
        case .denied: return "Switchboard needs the microphone and speech recognition. Turn them on in Settings, or type instead."
        case .unavailable: return "This iPhone can't transcribe your language on the device, so nothing was recorded. Type instead."
        }
    }
}

/// Apple's recognizer, on the device only: audio never leaves the phone.
@MainActor
final class SpeechListener: Listener {
    private let engine = AVAudioEngine()
    private var request: SFSpeechAudioBufferRecognitionRequest?
    private var task: SFSpeechRecognitionTask?

    func start(heard: @escaping @MainActor (String) -> Void, level: @escaping @MainActor (Float) -> Void) async throws {
        let speech = await Self.speechAuthorization()
        guard speech == .authorized, await AVAudioApplication.requestRecordPermission() else { throw VoiceError.denied }
        guard let recognizer = SFSpeechRecognizer(), recognizer.isAvailable, recognizer.supportsOnDeviceRecognition else {
            throw VoiceError.unavailable
        }
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.record, mode: .measurement, options: .duckOthers)
        try session.setActive(true, options: .notifyOthersOnDeactivation)

        let request = SFSpeechAudioBufferRecognitionRequest()
        request.requiresOnDeviceRecognition = true
        request.shouldReportPartialResults = true
        request.addsPunctuation = true
        self.request = request
        task = recognizer.recognitionTask(with: request, resultHandler: Self.resultHandler(heard))
        let input = engine.inputNode
        input.installTap(onBus: 0, bufferSize: 1024, format: input.outputFormat(forBus: 0), block: Self.tap(request, level))
        engine.prepare()
        try engine.start()
    }

    // These run on the recognizer's and the audio engine's own threads, so they're built outside
    // the main actor.

    private nonisolated static func speechAuthorization() async -> SFSpeechRecognizerAuthorizationStatus {
        await withCheckedContinuation { c in SFSpeechRecognizer.requestAuthorization { c.resume(returning: $0) } }
    }

    private nonisolated static func resultHandler(
        _ heard: @escaping @MainActor (String) -> Void
    ) -> @Sendable (SFSpeechRecognitionResult?, (any Error)?) -> Void {
        { result, _ in
            guard let words = result?.bestTranscription.formattedString else { return }
            Task { @MainActor in heard(words) }
        }
    }

    private nonisolated static func tap(
        _ request: SFSpeechAudioBufferRecognitionRequest, _ level: @escaping @MainActor (Float) -> Void
    ) -> AVAudioNodeTapBlock {
        nonisolated(unsafe) let request = request
        return { buffer, _ in
            request.append(buffer)
            guard let samples = buffer.floatChannelData?[0], buffer.frameLength > 0 else { return }
            var sum: Float = 0
            for i in 0..<Int(buffer.frameLength) { sum += samples[i] * samples[i] }
            let rms = (sum / Float(buffer.frameLength)).squareRoot()
            // About -50 dB is silence and -10 dB loud speech.
            let db = 20 * log10(max(rms, 1e-6))
            let l = min(1, max(0, (db + 50) / 40))
            Task { @MainActor in level(l) }
        }
    }

    func stop() {
        engine.stop()
        engine.inputNode.removeTap(onBus: 0)
        request?.endAudio()
        task?.cancel()
        request = nil
        task = nil
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }
}

/// Plays a phrase word by word, for UI tests and the simulator: `SB_FAKE_VOICE`.
@MainActor
final class ScriptedListener: Listener {
    private let words: [String]
    private var task: Task<Void, Never>?

    init(text: String) {
        words = text.split(separator: " ").map(String.init)
    }

    func start(heard: @escaping @MainActor (String) -> Void, level: @escaping @MainActor (Float) -> Void) async throws {
        let words = words
        task = Task {
            var tick = 0
            var said = 0
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(60))
                tick += 1
                let speaking = said < words.count
                level(speaking ? Float.random(in: 0.45...1) : Float.random(in: 0.05...0.15))
                if tick > 6, tick % 4 == 0, said < words.count {
                    said += 1
                    heard(words.prefix(said).joined(separator: " "))
                }
            }
        }
    }

    func stop() {
        task?.cancel()
    }
}
