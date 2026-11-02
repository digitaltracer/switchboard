import Foundation
import Network
import Testing

@testable import SwitchboardKit

// A real local HTTP server, so redirects and timeouts go through URLSession for real.
private final class FakeProvider: @unchecked Sendable {
    struct Seen {
        let head: String
        let body: Data
    }

    private let listener: NWListener
    private let queue = DispatchQueue(label: "fake-provider")
    private let lock = NSLock()
    private var seen: [Seen] = []
    private let answer: @Sendable (Int) -> String?

    /// `answer` gets the request's index and returns the raw response; nil never answers.
    init(answer: @escaping @Sendable (Int) -> String?) async throws {
        self.answer = answer
        listener = try NWListener(using: .tcp, on: .any)
        listener.newConnectionHandler = { [weak self] c in self?.accept(c) }
        let ready = AsyncStream<Void> { go in
            listener.stateUpdateHandler = { if case .ready = $0 { go.yield(); go.finish() } }
        }
        listener.start(queue: queue)
        for await _ in ready {}
    }

    deinit { listener.cancel() }

    var url: URL { URL(string: "http://127.0.0.1:\(listener.port!.rawValue)")! }
    var requests: [Seen] { lock.withLock { seen } }

    private func accept(_ c: NWConnection) {
        c.start(queue: queue)
        read(c, Data())
    }

    private func read(_ c: NWConnection, _ sofar: Data) {
        c.receive(minimumIncompleteLength: 1, maximumLength: 65_536) { [self] data, _, done, _ in
            let all = sofar + (data ?? Data())
            if let end = all.range(of: Data("\r\n\r\n".utf8)) {
                let head = String(decoding: all[..<end.lowerBound], as: UTF8.self)
                let length = head.split(separator: "\r\n")
                    .first { $0.lowercased().hasPrefix("content-length:") }
                    .flatMap { Int($0.split(separator: ":")[1].trimmingCharacters(in: .whitespaces)) } ?? 0
                if all.count - end.upperBound >= length {
                    respond(c, Seen(head: head, body: Data(all[end.upperBound...])))
                    return
                }
            }
            if !done { read(c, all) }
        }
    }

    private func respond(_ c: NWConnection, _ request: Seen) {
        let index = lock.withLock { seen.append(request); return seen.count - 1 }
        guard let raw = answer(index) else { return }
        c.send(content: Data(raw.utf8), completion: .contentProcessed { _ in c.cancel() })
    }
}

private func http(_ status: String, _ body: String, headers: String = "") -> String {
    "HTTP/1.1 \(status)\r\nContent-Type: application/json\r\nContent-Length: \(body.utf8.count)\r\nConnection: close\r\n\(headers)\r\n\(body)"
}

private func recording(_ bytes: String = "fake-audio") throws -> URL {
    let file = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).m4a")
    try Data(bytes.utf8).write(to: file)
    return file
}

private func client(_ server: FakeProvider, timeout: TimeInterval = 5) -> TranscriptionClient {
    TranscriptionClient(key: "sk-fake-test-key", baseURL: server.url, session: TranscriptionClient.makeSession(timeout: timeout))
}

@Test func theRequestCarriesTheModelTheNamesAndTheAudioWithTheKeyInTheHeader() async throws {
    let server = try await FakeProvider { _ in http("200 OK", #"{"text":" run the benchmarks "}"#) }
    let file = try recording()
    defer { try? FileManager.default.removeItem(at: file) }

    let text = try await client(server).transcribe(file: file, names: ["fix/retry-loop", "demo"])

    #expect(text == "run the benchmarks")
    let request = try #require(server.requests.first)
    #expect(request.head.hasPrefix("POST /v1/audio/transcriptions HTTP/1.1"))
    #expect(request.head.contains("Authorization: Bearer sk-fake-test-key"))
    #expect(request.head.contains("multipart/form-data; boundary=switchboard-"))
    let body = String(decoding: request.body, as: UTF8.self)
    #expect(body.contains("name=\"model\"\r\n\r\ngpt-transcribe\r\n"))
    #expect(body.contains("name=\"keywords[]\"\r\n\r\nfix/retry-loop\r\n"))
    #expect(body.contains("name=\"keywords[]\"\r\n\r\ndemo\r\n"))
    #expect(body.contains("filename=\"recording.m4a\"\r\nContent-Type: audio/mp4\r\n\r\nfake-audio\r\n"))
    #expect(!body.contains("sk-fake-test-key"))
}

@Test func theNamesAreBoundedAndWhatTheAPIRefusesIsLeftOut() {
    let many = (0..<150).map { "name-\($0)" }
    #expect(TranscriptionWire.keywords(many).count == Transcription.maxNames)
    let long = String(repeating: "x", count: Transcription.maxNameLength + 1)
    #expect(TranscriptionWire.keywords(["good", "a<b", "c>d", "e\nf", " ", long, "good", " padded "]) == ["good", "padded"])
}

@Test(arguments: [
    ("401 Unauthorized", #"{"error":{"code":"invalid_api_key"}}"#, TranscriptionError.auth),
    ("403 Forbidden", "{}", TranscriptionError.auth),
    ("429 Too Many Requests", #"{"error":{"code":"insufficient_quota"}}"#, TranscriptionError.quota),
    ("429 Too Many Requests", #"{"error":{"code":"rate_limit_exceeded"}}"#, TranscriptionError.rate),
    ("500 Internal Server Error", "oops", TranscriptionError.server),
    ("400 Bad Request", "{}", TranscriptionError.server),
    ("200 OK", "not json", TranscriptionError.malformed),
    ("200 OK", #"{"other":1}"#, TranscriptionError.malformed),
])
func eachFailureHasItsClass(status: String, body: String, expected: TranscriptionError) async throws {
    let server = try await FakeProvider { _ in http(status, body) }
    let file = try recording()
    defer { try? FileManager.default.removeItem(at: file) }
    await #expect(throws: expected) { try await client(server).transcribe(file: file, names: []) }
}

@Test func aRedirectIsNotFollowedSoTheKeyGoesNowhereElse() async throws {
    let elsewhere = try await FakeProvider { _ in http("200 OK", #"{"text":"stolen"}"#) }
    let location = elsewhere.url.absoluteString
    let server = try await FakeProvider { _ in http("307 Temporary Redirect", "", headers: "Location: \(location)/v1/audio/transcriptions\r\n") }
    let file = try recording()
    defer { try? FileManager.default.removeItem(at: file) }

    await #expect(throws: TranscriptionError.server) { try await client(server).transcribe(file: file, names: []) }

    #expect(server.requests.count == 1)
    #expect(elsewhere.requests.isEmpty)
}

@Test func noResponseInTimeIsATimeout() async throws {
    let server = try await FakeProvider { _ in nil }
    let file = try recording()
    defer { try? FileManager.default.removeItem(at: file) }
    await #expect(throws: TranscriptionError.timeout) { try await client(server, timeout: 0.3).transcribe(file: file, names: []) }
}

@Test func noServerIsOffline() {
    #expect(TranscriptionWire.error(URLError(.notConnectedToInternet)) == .offline)
    #expect(TranscriptionWire.error(URLError(.cannotConnectToHost)) == .offline)
    #expect(TranscriptionWire.error(URLError(.networkConnectionLost)) == .offline)
    #expect(TranscriptionWire.error(URLError(.timedOut)) == .timeout)
}

@Test func aMissingRecordingIsUnreadable() async throws {
    let server = try await FakeProvider { _ in http("200 OK", #"{"text":"x"}"#) }
    let gone = FileManager.default.temporaryDirectory.appending(path: "\(UUID().uuidString).m4a")
    await #expect(throws: TranscriptionError.unreadable) { try await client(server).transcribe(file: gone, names: []) }
    #expect(server.requests.isEmpty)
}

@Test func theKeyStaysOutOfEveryErrorMessage() {
    let all: [TranscriptionError] = [.offline, .timeout, .auth, .quota, .rate, .server, .malformed, .unreadable, .interrupted]
    for e in all { #expect(!e.message.contains("sk-")) }
    #expect(all.filter(\.rejectsKey) == [.auth, .quota])
}

// The gate, fed one reading every 100 ms.

private func run(_ gate: inout SpeechGate, _ level: Float, from: Double, to: Double) -> (SpeechGate.Decision, Double)? {
    var t = from
    while t < to {
        let d = gate.add(level: level, at: t)
        if d != .keepGoing { return (d, t) }
        t += 0.1
    }
    return nil
}

@Test func speechThenQuietStopsAfterThePause() throws {
    var gate = SpeechGate()
    #expect(run(&gate, -55, from: 0, to: 1) == nil)
    #expect(run(&gate, -20, from: 1, to: 4) == nil)
    let (decision, at) = try #require(run(&gate, -55, from: 4, to: 8))
    #expect(decision == .stop(heardSpeech: true))
    #expect(abs(at - (3.9 + SpeechGate.quietAfterSpeech)) < 0.11)
}

@Test func silenceStopsAtEightSecondsWithNoSpeech() throws {
    var gate = SpeechGate()
    let (decision, at) = try #require(run(&gate, -60, from: 0, to: 12))
    #expect(decision == .stop(heardSpeech: false))
    #expect(abs(at - SpeechGate.noSpeech) < 0.11)
}

@Test func unbrokenSpeechStopsAtTheCap() throws {
    var gate = SpeechGate()
    #expect(run(&gate, -60, from: 0, to: 0.5) == nil)
    var t = 0.5
    var outcome: (SpeechGate.Decision, Double)?
    while t < 100, outcome == nil {
        // Syllables with short gaps, loud enough that the rising floor never reaches them.
        let level: Float = Int(t * 10) % 5 == 0 ? -40 : -15
        let d = gate.add(level: level, at: t)
        if d != .keepGoing { outcome = (d, t) }
        t += 0.1
    }
    let (decision, at) = try #require(outcome)
    #expect(decision == .stop(heardSpeech: true))
    #expect(abs(at - SpeechGate.cap) < 0.11)
}

@Test func aSteadyNoisyRoomIsNotSpeechButTalkingOverItIs() throws {
    var noisy = SpeechGate()
    let (decision, _) = try #require(run(&noisy, -40, from: 0, to: 12))
    #expect(decision == .stop(heardSpeech: false))

    var talking = SpeechGate()
    #expect(run(&talking, -40, from: 0, to: 2) == nil)
    #expect(run(&talking, -15, from: 2, to: 4) == nil)
    #expect(talking.heardSpeech)
    let (stopped, _) = try #require(run(&talking, -40, from: 4, to: 8))
    #expect(stopped == .stop(heardSpeech: true))
}

@Test func aKnockIsNotSpeech() throws {
    var gate = SpeechGate()
    #expect(run(&gate, -60, from: 0, to: 2) == nil)
    #expect(gate.add(level: -10, at: 2.0) == .keepGoing)
    #expect(gate.add(level: -60, at: 2.1) == .keepGoing)
    #expect(!gate.heardSpeech)
}

@Test func speakingFromTheFirstInstantCounts() throws {
    var gate = SpeechGate()
    #expect(run(&gate, -20, from: 0, to: 1) == nil)
    #expect(gate.heardSpeech)
}

@Test func aRecordersEmptyFirstReadingDoesNotSetTheFloor() throws {
    var gate = SpeechGate()
    #expect(gate.add(level: -160, at: 0) == .keepGoing)
    // The room is -40: with a floor of -160 it would pass as speech.
    let (decision, _) = try #require(run(&gate, -40, from: 0.1, to: 12))
    #expect(decision == .stop(heardSpeech: false))
}

// The lifecycle, with a fake transport and a fake clock.

private final class FakeTransport: TranscriptionTransport, @unchecked Sendable {
    private let lock = NSLock()
    private var waiting: [CheckedContinuation<String, Error>] = []
    private var answers: [Result<String, Error>]
    private(set) var sentNames: [[String]] = []
    private var calls = 0

    /// With no answers given, each call waits for `answer(_:)`.
    init(_ answers: [Result<String, Error>] = []) { self.answers = answers }

    var callCount: Int { lock.withLock { calls } }
    var pending: Int { lock.withLock { waiting.count } }

    func transcribe(file _: URL, names: [String]) async throws -> String {
        try await withCheckedThrowingContinuation { c in
            lock.withLock {
                calls += 1
                sentNames.append(names)
                if answers.isEmpty {
                    waiting.append(c)
                } else {
                    c.resume(with: answers.removeFirst())
                }
            }
        }
    }

    func answer(_ result: Result<String, Error>) {
        lock.withLock { if !waiting.isEmpty { waiting.removeFirst().resume(with: result) } }
    }
}

@MainActor
private final class Harness {
    var time = Date(timeIntervalSince1970: 1_000)
    var deleted: [URL] = []
    let file = URL(fileURLWithPath: "/tmp/rec.m4a")
    lazy var life = RecordingLifecycle(now: { [unowned self] in time }, delete: { [unowned self] in deleted.append($0) })
}

private func untilPending(_ fake: FakeTransport, _ count: Int) async {
    while fake.pending < count { await Task.yield() }
}

@MainActor @Test func successDeletesTheFile() async {
    let h = Harness()
    let outcome = await h.life.start(file: h.file, transport: FakeTransport([.success("hello")]), names: ["demo"])
    #expect(outcome == .text("hello"))
    #expect(h.deleted == [h.file])
    #expect(!h.life.canRetry)
}

@MainActor @Test func aFailureKeepsTheFileAndRetrySendsTheSameOneOnlyWhenAsked() async {
    let h = Harness()
    let fake = FakeTransport([.failure(TranscriptionError.server), .success("second time")])
    let first = await h.life.start(file: h.file, transport: fake, names: ["demo"])
    #expect(first == .kept(.server))
    #expect(h.deleted.isEmpty)
    #expect(h.life.canRetry)
    #expect(fake.callCount == 1, "nothing resends by itself")

    let again = await h.life.retry()
    #expect(again == .text("second time"))
    #expect(fake.sentNames == [["demo"], ["demo"]])
    #expect(h.deleted == [h.file])
}

@MainActor @Test func aRejectedKeyDeletesTheFile() async {
    for error in [TranscriptionError.auth, .quota, .unreadable] {
        let h = Harness()
        let outcome = await h.life.start(file: h.file, transport: FakeTransport([.failure(error)]), names: [])
        #expect(outcome == .lost(error))
        #expect(h.deleted == [h.file])
        #expect(!h.life.canRetry)
    }
}

@MainActor @Test func aKeptFileExpiresAfterTenMinutesEvenAcrossRetries() async {
    let h = Harness()
    let fake = FakeTransport([.failure(TranscriptionError.timeout), .failure(TranscriptionError.timeout)])
    _ = await h.life.start(file: h.file, transport: fake, names: [])
    #expect(h.life.expiresAt == h.time.addingTimeInterval(600))

    h.time.addTimeInterval(300)
    #expect(!h.life.expireIfDue())
    _ = await h.life.retry()
    #expect(h.life.expiresAt == Date(timeIntervalSince1970: 1_600), "a retry doesn't extend it")

    h.time.addTimeInterval(300)
    let late = await h.life.retry()
    #expect(late == .expired)
    #expect(h.deleted == [h.file])
    #expect(fake.callCount == 2, "an expired recording isn't sent")
}

@MainActor @Test func theExpiryTimerDeletesTheFileWithNoTap() async {
    let h = Harness()
    _ = await h.life.start(file: h.file, transport: FakeTransport([.failure(TranscriptionError.server)]), names: [])
    h.time.addTimeInterval(601)
    #expect(h.life.expireIfDue())
    #expect(h.deleted == [h.file])
    #expect(!h.life.canRetry)
}

@MainActor @Test func aSheetExitDeletesAKeptFile() async {
    let h = Harness()
    _ = await h.life.start(file: h.file, transport: FakeTransport([.failure(TranscriptionError.server)]), names: [])
    h.life.discard()
    #expect(h.deleted == [h.file])
    h.life.discard()
    #expect(h.deleted == [h.file], "only once")
}

@MainActor @Test func aNewRecordingDeletesTheKeptOne() async {
    let h = Harness()
    _ = await h.life.start(file: h.file, transport: FakeTransport([.failure(TranscriptionError.server)]), names: [])
    let next = URL(fileURLWithPath: "/tmp/next.m4a")
    let outcome = await h.life.start(file: next, transport: FakeTransport([.failure(TranscriptionError.server)]), names: [])
    #expect(outcome == .kept(.server))
    #expect(h.deleted == [h.file])
    #expect(h.life.canRetry)
}

@MainActor @Test func aDoubleRetryIgnoresTheFirstAnswer() async {
    let h = Harness()
    let fake = FakeTransport()
    let first = Task { await h.life.start(file: h.file, transport: fake, names: []) }
    await untilPending(fake, 1)
    fake.answer(.failure(TranscriptionError.server))
    #expect(await first.value == .kept(.server))

    let one = Task { await h.life.retry() }
    await untilPending(fake, 1)
    let two = Task { await h.life.retry() }
    while fake.callCount < 3 { await Task.yield() }

    fake.answer(.success("from the first retry"))
    #expect(await one.value == .discarded)
    #expect(h.deleted.isEmpty, "the second retry still needs the file")

    fake.answer(.success("from the second retry"))
    #expect(await two.value == .text("from the second retry"))
    #expect(h.deleted == [h.file])
}

@MainActor @Test func dismissingDuringARetryDiscardsTheAnswer() async {
    let h = Harness()
    let fake = FakeTransport([.failure(TranscriptionError.server)])
    _ = await h.life.start(file: h.file, transport: fake, names: [])

    let again = Task { await h.life.retry() }
    await untilPending(fake, 1)
    h.life.discard()
    fake.answer(.success("too late"))
    #expect(await again.value == .discarded)
    #expect(h.deleted == [h.file])
    #expect(!h.life.canRetry)
    #expect(!h.life.uploading)
}

@MainActor @Test func aNewListenDuringARetryDiscardsTheAnswerAndKeepsOnlyTheNewFile() async {
    let h = Harness()
    let old = FakeTransport()
    let first = Task { await h.life.start(file: h.file, transport: old, names: []) }
    await untilPending(old, 1)

    let next = URL(fileURLWithPath: "/tmp/next.m4a")
    let new = FakeTransport()
    let second = Task { await h.life.start(file: next, transport: new, names: []) }
    await untilPending(new, 1)
    old.answer(.success("old words"))
    #expect(await first.value == .discarded)
    #expect(h.deleted == [h.file])

    new.answer(.success("new words"))
    #expect(await second.value == .text("new words"))
    #expect(h.deleted == [h.file, next])
}

@MainActor @Test func goingToTheBackgroundStopsTheUploadAndKeepsTheFileForRetry() async {
    let h = Harness()
    let fake = FakeTransport()
    let upload = Task { await h.life.start(file: h.file, transport: fake, names: []) }
    await untilPending(fake, 1)

    h.life.interrupt()
    #expect(h.life.canRetry)
    #expect(h.deleted.isEmpty)
    fake.answer(.success("too late"))
    #expect(await upload.value == .discarded)

    let again = Task { await h.life.retry() }
    await untilPending(fake, 1)
    fake.answer(.success("back again"))
    #expect(await again.value == .text("back again"))
    #expect(h.deleted == [h.file])
}

@MainActor @Test func interruptingWithNoUploadChangesNothing() {
    let h = Harness()
    h.life.interrupt()
    #expect(!h.life.canRetry)
    #expect(h.life.expiresAt == nil)
}

@Test func theLaunchSweepClearsWhatACrashLeft() throws {
    let dir = FileManager.default.temporaryDirectory.appending(path: UUID().uuidString)
    let store = RecordingStore(directory: dir)
    let file = try store.newFile()
    #expect(file.pathExtension == "m4a")
    try Data("left behind".utf8).write(to: file)
    store.sweep()
    #expect(!FileManager.default.fileExists(atPath: file.path))
    store.sweep()
    #expect(try store.newFile().deletingLastPathComponent().path == dir.path)
    store.sweep()
}
