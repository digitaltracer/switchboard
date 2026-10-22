import CryptoKit
import Foundation
import HTTPTypes
import OpenAPIRuntime
import Testing

@testable import SwitchboardKit

@Test func messagesMatchTheHubs() {
    // The same strings `src/auth/device_tests.rs` pins.
    #expect(SignedMessage.approval(hub: "h1", device: "d1", approval: "a1", nonce: "n1", expiresAt: 42)
        == "switchboard-approval-v1\nhub=h1\ndevice=d1\napproval=a1\ndecision=allow\nnonce=n1\nexpires=42")
    #expect(SignedMessage.token(hub: "h1", device: "d1", nonce: "n1", expiresAt: 42)
        == "switchboard-token-v1\nhub=h1\ndevice=d1\nnonce=n1\nexpires=42")
    #expect(pairingFingerprint(hubId: "h1", authKey: "k1", approvalKey: "k2") == "a553-4998-583d-047b-1245-3ce8")
}

@Test func pairingLinksAreParsedStrictly() {
    let link = PairingLink("switchboard://pair?url=https%3A%2F%2Fhub.example.com&code=c0de&hub=h1")
    #expect(link?.hubURL == URL(string: "https://hub.example.com"))
    // The hub's own link ends its URL with a slash; requests must not start with `//`.
    let slashed = PairingLink("switchboard://pair?url=http%3A%2F%2F127.0.0.1%3A8080%2F&code=c&hub=h")
    #expect(slashed?.hubURL.absoluteString == "http://127.0.0.1:8080")
    #expect(link?.code == "c0de")
    #expect(link?.hubId == "h1")
    #expect(PairingLink("https://pair?url=x&code=c&hub=h") == nil)
    #expect(PairingLink("switchboard://pair?url=ftp%3A%2F%2Fx&code=c&hub=h") == nil)
    #expect(PairingLink("switchboard://pair?url=https%3A%2F%2Fx&hub=h") == nil)
}

@Test func countdownsShowMinutesAndSeconds() {
    #expect(Countdown.text(deadline: 10_000 + 275_000, now: 10_000) == "4:35")
    #expect(Countdown.text(deadline: 1_000, now: 5_000) == "0:00")
    #expect(Countdown.text(deadline: 61_999, now: 1_000) == "1:00")
}

/// A scripted hub: answers by operation id and records what it was sent.
final class FakeHub: ClientTransport, @unchecked Sendable {
    var requests: [(op: String, request: HTTPRequest, body: Data)] = []
    var handler: (String, HTTPRequest, Data) -> (Int, String)
    private let lock = NSLock()

    init(_ handler: @escaping (String, HTTPRequest, Data) -> (Int, String)) {
        self.handler = handler
    }

    func send(_ request: HTTPRequest, body: HTTPBody?, baseURL: URL, operationID: String) async throws -> (HTTPResponse, HTTPBody?) {
        var data = Data()
        if let body { data = try await Data(collecting: body, upTo: 1 << 20) }
        let (status, json) = lock.withLock {
            requests.append((operationID, request, data))
            return handler(operationID, request, data)
        }
        var response = HTTPResponse(status: .init(code: status))
        response.headerFields[.contentType] = "application/json"
        return (response, HTTPBody(json))
    }

    func ops() -> [String] {
        lock.withLock { requests.map(\.op) }
    }
}

let pairing = Pairing(hubURL: URL(string: "https://hub.example.com")!, hubId: "h1", deviceId: "d1", name: "iPhone")

func tokenAnswers(_ op: String) -> (Int, String)? {
    switch op {
    case "token_challenge":
        return (200, #"{"nonce":"n1","expires_at":42,"message_version":1,"hub_id":"h1","device_id":"d1"}"#)
    case "issue_token":
        return (200, #"{"access_token":"t-\#(UUID().uuidString)","expires_at":\#(Int64(Date().timeIntervalSince1970 * 1000) + 900_000)}"#)
    default:
        return nil
    }
}

@Test func errorsComeBackAsTheHubsCodes() async throws {
    let fake = FakeHub { op, _, _ in
        tokenAnswers(op) ?? (409, #"{"error":{"code":"answer_in_herdr","message":"answer it in Herdr"}}"#)
    }
    let slashed = Pairing(hubURL: URL(string: "https://hub.example.com/")!, hubId: "h1", deviceId: "d1", name: "iPhone")
    let hub = Hub(pairing: slashed, keys: SoftwareKeys(), build: 7, transport: fake)
    #expect(hub.baseURL.absoluteString == "https://hub.example.com")
    do {
        _ = try await hub.deny("a1", requestId: "r1")
        Issue.record("expected an error")
    } catch let e as HubError {
        #expect(e.code == "answer_in_herdr")
    }
    let sent = fake.requests.first { $0.op == "decide_approval" }!
    #expect(sent.request.headerFields[HTTPField.Name("x-switchboard-build")!] == "7")
    #expect(sent.request.headerFields[.authorization]?.hasPrefix("Bearer t-") == true)
}

@Test func tokensAreSignedWithTheAuthKeyAndRefreshedOnce() async throws {
    let keys = SoftwareKeys()
    var unauthorizedOnce = true
    let fake = FakeHub { op, _, _ in
        if let t = tokenAnswers(op) { return t }
        if unauthorizedOnce {
            unauthorizedOnce = false
            return (401, #"{"error":{"code":"invalid_token","message":"expired"}}"#)
        }
        return (200, #"[]"#)
    }
    let hub = Hub(pairing: pairing, keys: keys, build: 1, transport: fake)
    let projects = try await hub.projects()
    #expect(projects.isEmpty)
    #expect(fake.ops() == ["token_challenge", "issue_token", "list_projects", "token_challenge", "issue_token", "list_projects"])
    let issue = try JSONSerialization.jsonObject(with: fake.requests[1].body) as! [String: Any]
    let der = Data(base64URL: issue["signature"] as! String)!
    let key = try P256.Signing.PublicKey(x963Representation: try keys.publicKey(.auth))
    let message = SignedMessage.token(hub: "h1", device: "d1", nonce: "n1", expiresAt: 42)
    #expect(key.isValidSignature(try P256.Signing.ECDSASignature(derRepresentation: der), for: Data(message.utf8)))
}

@Test func approvingSignsTheChallengeWithTheApprovalKey() async throws {
    let keys = SoftwareKeys()
    let fake = FakeHub { op, _, _ in
        if let t = tokenAnswers(op) { return t }
        switch op {
        case "approval_challenge":
            return (200, #"{"nonce":"an","expires_at":99,"message_version":1,"hub_id":"h1","device_id":"d1","approval_id":"a1"}"#)
        default:
            return (200, #"{"session_id":"s1","approval_id":"a1","outcome":"allow","delivery":"delivered","replayed":false}"#)
        }
    }
    let hub = Hub(pairing: pairing, keys: keys, build: 1, transport: fake)
    let r = try await hub.approve("a1", reason: "Approve Bash", requestId: "r1")
    #expect(r.outcome == "allow")
    let body = try JSONSerialization.jsonObject(with: fake.requests.last!.body) as! [String: Any]
    #expect(body["decision"] as? String == "allow")
    #expect(body["nonce"] as? String == "an")
    let sig = try P256.Signing.ECDSASignature(derRepresentation: Data(base64URL: body["signature"] as! String)!)
    let approvalKey = try P256.Signing.PublicKey(x963Representation: try keys.publicKey(.approval))
    let authKey = try P256.Signing.PublicKey(x963Representation: try keys.publicKey(.auth))
    let message = Data(SignedMessage.approval(hub: "h1", device: "d1", approval: "a1", nonce: "an", expiresAt: 99).utf8)
    #expect(approvalKey.isValidSignature(sig, for: message))
    #expect(!authKey.isValidSignature(sig, for: message))
}

@Test func aLostStartIsLookedUpByItsRequestId() async throws {
    let fake = FakeHub { op, _, _ in
        if let t = tokenAnswers(op) { return t }
        switch op {
        case "start_session":
            return (504, #"{"error":{"code":"outcome_unknown","message":"lost","operation_id":"r9","session_id":"s9"}}"#)
        default:
            return (200, #"{"request_id":"r9","kind":"session_start","state":"done","session_id":"s9"}"#)
        }
    }
    let hub = Hub(pairing: pairing, keys: SoftwareKeys(), build: 1, transport: fake)
    let state = await StartFlow.run(hub: hub, project: "p", prompt: "x", machine: nil, agent: nil, notify: true, requestId: "r9")
    #expect(state == .unknown(requestId: "r9"))
    let settled = await StartFlow.reconcile(hub: hub, requestId: "r9")
    #expect(settled == .started(sessionId: "s9", alreadyRunning: false))
}

@Test func aRequestNotYetSeenIsGivenTimeBeforeItCountsAsNeverSent() async throws {
    let fake = FakeHub { op, _, _ in
        if let t = tokenAnswers(op) { return t }
        return (404, #"{"error":{"code":"unknown_operation","message":"no request with this id reached the hub"}}"#)
    }
    let hub = Hub(pairing: pairing, keys: SoftwareKeys(), build: 1, transport: fake)
    #expect(await StartFlow.reconcile(hub: hub, requestId: "r1", attempt: 0) == .unknown(requestId: "r1"))
    if case .failed = await StartFlow.reconcile(hub: hub, requestId: "r1", attempt: 2) {} else {
        Issue.record("after a few looks it never reached the hub")
    }
}

@Test func pushTokensUseTheEnvironmentTheBuildWasSignedFor() {
    func profile(_ env: String) -> Data {
        let plist = """
        <?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>Entitlements</key>\
        <dict><key>aps-environment</key><string>\(env)</string></dict></dict></plist>
        """
        // The plist sits between the binary parts of a signed envelope.
        return Data([0x30, 0x82, 0x01]) + Data(plist.utf8) + Data([0xA0, 0x00])
    }
    #expect(APNsEnvironment.apsEnvironment(profile: profile("development")) == "development")
    #expect(APNsEnvironment.isSandbox(profile: profile("development"), simulator: false))
    #expect(!APNsEnvironment.isSandbox(profile: profile("production"), simulator: false), "ad hoc")
    #expect(!APNsEnvironment.isSandbox(profile: nil, simulator: false), "TestFlight and the App Store carry no profile")
    #expect(APNsEnvironment.isSandbox(profile: nil, simulator: true))
}
