import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// Cloud voice: one recording format, one model, one host.
public enum Transcription {
    public static let model = "gpt-transcribe"
    public static let sampleRate = 16_000
    public static let channels = 1
    public static let bitRate = 32_000
    public static let fileExtension = "m4a"
    /// No response in this long counts as a failed upload.
    public static let uploadTimeout: TimeInterval = 20
    /// How long a failed recording may wait for Retry.
    public static let keptFor: TimeInterval = 600
    public static let maxNames = 100
    public static let maxNameLength = 100
}

public enum TranscriptionError: Error, Equatable, Sendable {
    case offline
    case timeout
    case auth
    case quota
    case rate
    case server
    case malformed
    /// The recording can't be read, so there's nothing to send again.
    case unreadable
    /// The app left the foreground mid-upload.
    case interrupted

    /// A rejected key or an exhausted account: resending can't help until the key changes.
    public var rejectsKey: Bool { self == .auth || self == .quota }

    public var message: String {
        switch self {
        case .offline: "No connection reached OpenAI. Your recording is kept for now, so tap Retry."
        case .timeout: "OpenAI didn't answer in time. Your recording is kept for now, so tap Retry."
        case .rate: "OpenAI is limiting requests right now. Your recording is kept for now, so tap Retry."
        case .server: "OpenAI couldn't transcribe that. Your recording is kept for now, so tap Retry."
        case .malformed: "OpenAI's answer couldn't be read. Your recording is kept for now, so tap Retry."
        case .interrupted: "The upload stopped when Switchboard left the screen. Your recording is kept for now, so tap Retry."
        case .auth: "OpenAI refused this phone's key, so the recording was deleted. Voice stays on this phone until you replace the key in Settings."
        case .quota: "This OpenAI key is out of quota, so the recording was deleted. Voice stays on this phone until you replace the key in Settings."
        case .unreadable: "The recording is gone. Say it again, or type instead."
        }
    }
}

/// The request's body and the answer's text. Pure, so the shape can be tested on its own.
public enum TranscriptionWire {
    /// Letters, digits and hyphens only, so it can't appear in a header or a name.
    public static func boundary() -> String { "switchboard-\(UUID().uuidString)" }

    /// The API refuses a whole request for a keyword with `<`, `>` or a line break.
    public static func keywords(_ names: [String]) -> [String] {
        var seen = Set<String>()
        var out: [String] = []
        for name in names {
            let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty, trimmed.count <= Transcription.maxNameLength,
                  trimmed.rangeOfCharacter(from: CharacterSet(charactersIn: "<>\r\n")) == nil,
                  seen.insert(trimmed).inserted else { continue }
            out.append(trimmed)
            if out.count == Transcription.maxNames { break }
        }
        return out
    }

    public static func body(audio: Data, names: [String], boundary: String) -> Data {
        var body = Data()
        func field(_ name: String, _ value: String) {
            body.append(Data("--\(boundary)\r\nContent-Disposition: form-data; name=\"\(name)\"\r\n\r\n\(value)\r\n".utf8))
        }
        field("model", Transcription.model)
        field("response_format", "json")
        for keyword in keywords(names) { field("keywords[]", keyword) }
        body.append(Data("--\(boundary)\r\nContent-Disposition: form-data; name=\"file\"; filename=\"recording.\(Transcription.fileExtension)\"\r\nContent-Type: audio/mp4\r\n\r\n".utf8))
        body.append(audio)
        body.append(Data("\r\n--\(boundary)--\r\n".utf8))
        return body
    }

    public static func text(from data: Data) throws -> String {
        struct Answer: Decodable { let text: String }
        guard let answer = try? JSONDecoder().decode(Answer.self, from: data) else { throw TranscriptionError.malformed }
        return answer.text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// A status other than 200, by what the owner can do about it. A redirect is never followed.
    public static func error(status: Int, body: Data) -> TranscriptionError {
        struct Failure: Decodable {
            struct Detail: Decodable { let code: String? }
            let error: Detail?
        }
        switch status {
        case 401, 403: return .auth
        case 429:
            let code = (try? JSONDecoder().decode(Failure.self, from: body))?.error?.code
            return code == "insufficient_quota" ? .quota : .rate
        default: return .server
        }
    }

    public static func error(_ url: URLError) -> TranscriptionError {
        switch url.code {
        case .timedOut: .timeout
        case .notConnectedToInternet, .networkConnectionLost, .cannotFindHost, .cannotConnectToHost,
             .dnsLookupFailed, .dataNotAllowed, .internationalRoamingOff, .callIsActive:
            .offline
        default: .server
        }
    }
}

public protocol TranscriptionTransport: Sendable {
    func transcribe(file: URL, names: [String]) async throws -> String
}

private final class NoRedirects: NSObject, URLSessionTaskDelegate, Sendable {
    func urlSession(
        _: URLSession, task _: URLSessionTask, willPerformHTTPRedirection _: HTTPURLResponse,
        newRequest _: URLRequest, completionHandler: @escaping @Sendable (URLRequest?) -> Void
    ) {
        completionHandler(nil)
    }
}

/// Uploads straight to OpenAI on its own session, with no redirects, cookies or cache. It shares
/// nothing with the hub's client, and the app can't point it anywhere else.
public struct TranscriptionClient: TranscriptionTransport {
    private static let provider = URL(string: "https://api.openai.com")!
    private static let session = makeSession(timeout: Transcription.uploadTimeout)

    private let key: String
    private let baseURL: URL
    private let session: URLSession

    public init(key: String) {
        self.init(key: key, baseURL: Self.provider, session: Self.session)
    }

    init(key: String, baseURL: URL, session: URLSession) {
        self.key = key
        self.baseURL = baseURL
        self.session = session
    }

    static func makeSession(timeout: TimeInterval) -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = timeout
        config.timeoutIntervalForResource = timeout
        config.waitsForConnectivity = false
        config.httpCookieStorage = nil
        config.urlCache = nil
        config.urlCredentialStorage = nil
        return URLSession(configuration: config, delegate: NoRedirects(), delegateQueue: nil)
    }

    public func transcribe(file: URL, names: [String]) async throws -> String {
        guard let audio = try? Data(contentsOf: file) else { throw TranscriptionError.unreadable }
        let boundary = TranscriptionWire.boundary()
        var request = URLRequest(url: baseURL.appending(path: "v1/audio/transcriptions"))
        request.httpMethod = "POST"
        request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        let body = TranscriptionWire.body(audio: audio, names: names, boundary: boundary)
        do {
            let (data, response) = try await session.upload(for: request, from: body)
            guard let status = (response as? HTTPURLResponse)?.statusCode else { throw TranscriptionError.malformed }
            guard status == 200 else { throw TranscriptionWire.error(status: status, body: data) }
            return try TranscriptionWire.text(from: data)
        } catch let e as URLError where e.code == .cancelled {
            throw CancellationError()
        } catch let e as URLError {
            throw TranscriptionWire.error(e)
        }
    }
}

/// When a cloud listen ends, from loudness alone: batch transcription has no partial text to go by.
/// Levels are in dBFS. The floor follows the room, so a noisy room isn't taken for speech.
public struct SpeechGate: Sendable {
    public enum Decision: Equatable, Sendable {
        case keepGoing
        case stop(heardSpeech: Bool)
    }

    public static let quietAfterSpeech: TimeInterval = 1.8
    public static let noSpeech: TimeInterval = 8
    public static let cap: TimeInterval = 90
    /// Speech must hold this long, so a knock doesn't count.
    static let onset: TimeInterval = 0.2
    static let speechAboveFloor: Float = 12
    static let stillTalkingAboveFloor: Float = 6
    static let quietestSpeech: Float = -55
    /// A floor this high or lower at the start lets someone who speaks at once be heard.
    static let startingFloor: Float = -45
    static let floorRisePerSecond: Float = 1
    /// Below this is no audio at all, such as a recorder's first reading.
    static let digitalSilence: Float = -85

    public private(set) var heardSpeech = false
    private var startedAt: TimeInterval?
    private var lastAt: TimeInterval?
    private var floor: Float?
    private var loudSince: TimeInterval?
    private var lastSpeech: TimeInterval = 0

    public init() {}

    public mutating func add(level: Float, at now: TimeInterval) -> Decision {
        let started = startedAt ?? now
        startedAt = started
        let elapsed = lastAt.map { Float(max(0, now - $0)) } ?? 0
        lastAt = now
        if level > Self.digitalSilence {
            floor = floor.map { min(level, $0 + Self.floorRisePerSecond * elapsed) } ?? min(level, Self.startingFloor)
        }
        let room = floor ?? Self.startingFloor
        if level > max(room + Self.speechAboveFloor, Self.quietestSpeech) {
            let since = loudSince ?? now
            loudSince = since
            if !heardSpeech, now - since >= Self.onset {
                heardSpeech = true
                lastSpeech = now
            }
        } else {
            loudSince = nil
        }
        if heardSpeech, level > max(room + Self.stillTalkingAboveFloor, Self.quietestSpeech) { lastSpeech = now }

        if now - started >= Self.cap { return .stop(heardSpeech: heardSpeech) }
        if heardSpeech { return now - lastSpeech >= Self.quietAfterSpeech ? .stop(heardSpeech: true) : .keepGoing }
        return now - started >= Self.noSpeech ? .stop(heardSpeech: false) : .keepGoing
    }

    /// For the level bars: about -50 dB is silence and -10 dB loud speech.
    public static func barLevel(_ level: Float) -> Float {
        min(1, max(0, (level + 50) / 40))
    }
}

/// Where recordings live: the app's temporary storage, which is never backed up.
public struct RecordingStore: Sendable {
    public let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    public func newFile() throws -> URL {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory.appending(path: "\(UUID().uuidString).\(Transcription.fileExtension)")
    }

    /// Clears whatever a crash left behind.
    public func sweep() {
        try? FileManager.default.removeItem(at: directory)
    }
}

/// One recording from upload to deletion. A failed upload keeps the file for Retry, for at most
/// `Transcription.keptFor`. Anything else that ends it deletes the file, and a later call makes
/// an earlier upload's answer count for nothing.
@MainActor
public final class RecordingLifecycle {
    public enum Outcome: Equatable, Sendable {
        /// The file is deleted. The text may be empty.
        case text(String)
        /// The file is kept for Retry.
        case kept(TranscriptionError)
        /// The file is deleted: resending can't help.
        case lost(TranscriptionError)
        /// A later call took over, so this answer is ignored.
        case discarded
        case expired
    }

    private let now: @MainActor () -> Date
    private let delete: @MainActor (URL) -> Void
    private var file: URL?
    private var transport: TranscriptionTransport?
    private var names: [String] = []
    private var keptSince: Date?
    private var generation = 0
    private var inFlight: Task<Result<String, TranscriptionError>, Never>?

    public init(
        now: @escaping @MainActor () -> Date = { Date() },
        delete: @escaping @MainActor (URL) -> Void = { try? FileManager.default.removeItem(at: $0) }
    ) {
        self.now = now
        self.delete = delete
    }

    /// A recording is waiting for Retry, with no upload running.
    public var canRetry: Bool { file != nil && keptSince != nil && inFlight == nil }
    public var uploading: Bool { inFlight != nil }
    public var expiresAt: Date? { keptSince?.addingTimeInterval(Transcription.keptFor) }

    /// A new recording replaces any earlier one and uploads.
    public func start(file: URL, transport: TranscriptionTransport, names: [String]) async -> Outcome {
        discard()
        self.file = file
        self.transport = transport
        self.names = names
        return await send()
    }

    /// Sends the same recording again. A second tap supersedes the first.
    public func retry() async -> Outcome {
        if expireIfDue() { return .expired }
        guard file != nil else { return .expired }
        return await send()
    }

    /// The app is leaving the foreground: the upload stops and the file stays for Retry.
    public func interrupt() {
        guard inFlight != nil else { return }
        stopUpload()
        keptSince = keptSince ?? now()
    }

    /// Any exit: deletes the file and ignores whatever is still in flight.
    public func discard() {
        stopUpload()
        if let file { delete(file) }
        file = nil
        transport = nil
        names = []
        keptSince = nil
    }

    /// True if the kept recording has just passed its limit and was deleted.
    @discardableResult
    public func expireIfDue() -> Bool {
        guard let expiresAt, now() >= expiresAt else { return false }
        discard()
        return true
    }

    private func stopUpload() {
        generation += 1
        inFlight?.cancel()
        inFlight = nil
    }

    private func send() async -> Outcome {
        guard let file, let transport else { return .expired }
        stopUpload()
        let mine = generation
        let names = names
        let task = Task<Result<String, TranscriptionError>, Never> {
            do {
                return .success(try await transport.transcribe(file: file, names: names))
            } catch let e as TranscriptionError {
                return .failure(e)
            } catch {
                return .failure(.interrupted)
            }
        }
        inFlight = task
        let result = await task.value
        guard mine == generation else { return .discarded }
        inFlight = nil
        switch result {
        case .success(let text):
            discard()
            return .text(text)
        case .failure(let error) where error.rejectsKey || error == .unreadable:
            discard()
            return .lost(error)
        case .failure(let error):
            keptSince = keptSince ?? now()
            return .kept(error)
        }
    }
}
