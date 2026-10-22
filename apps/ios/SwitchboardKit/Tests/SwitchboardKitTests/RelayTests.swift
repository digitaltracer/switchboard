import CryptoKit
import Foundation
import HTTPTypes
import OpenAPIRuntime
import Testing

@testable import SwitchboardKit

/// `tests/fixtures/app-v1/relay-vector.json`, shared with the hub's Rust tests.
func relayVector() throws -> [String: Any] {
    let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    let data = try Data(contentsOf: root.appending(path: "tests/fixtures/app-v1/relay-vector.json"))
    return try JSONSerialization.jsonObject(with: data) as! [String: Any]
}

func hexData(_ s: String) -> Data {
    Data(stride(from: 0, to: s.count, by: 2).map { i in
        UInt8(s[s.index(s.startIndex, offsetBy: i)..<s.index(s.startIndex, offsetBy: i + 2)], radix: 16)!
    })
}

@Test func theHubsRustEnvelopesInteroperate() throws {
    let v = try relayVector()
    let hub = try P256.KeyAgreement.PrivateKey(rawRepresentation: hexData(v["hub_secret_hex"] as! String))
    let eph = try P256.KeyAgreement.PrivateKey(rawRepresentation: hexData(v["phone_secret_hex"] as! String))
    #expect(hub.publicKey.x963Representation.base64URL == v["hub_kx_public"] as! String)
    let keys = try RelaySeal.keys(eph: eph, hub: hub.publicKey)

    // The same request the fixture's Swift half sealed.
    let plain = Data(#"{"body":"","headers":[["x-switchboard-build","1"]],"method":"GET","path":"/app/v1/meta","ts":42}"#.utf8)
    let request = v["swift_request"] as! [String: Any]
    #expect(try RelaySeal.seal(plain, key: keys.request).base64URL == request["ct"] as! String)

    // The hub's Rust reply opens with this phone's reply key.
    let reply = v["rust_response"] as! [String: Any]
    let opened = try RelaySeal.open(Data(base64URL: reply["ct"] as! String)!, key: keys.reply,
                                    nonce: Data(base64URL: reply["n"] as! String)!)
    let answer = try JSONDecoder().decode(RelaySeal.PlainResponse.self, from: opened)
    #expect(answer.status == 200)
    #expect(Data(base64Encoded: answer.body) == Data(#"{"ok":true}"#.utf8))

    // The relay id is the hub's, from both keys.
    let route = RelayRoute(relayURL: URL(string: "https://adarshnb.com/relay/")!, hubKey: v["hub_kx_public"] as! String, signKey: v["sign_public"] as! String)
    #expect(route?.relayId == v["relay_id"] as? String)
    #expect(route?.relayURL.absoluteString == "https://adarshnb.com/relay")
}

@Test func relayPairingLinksCarryTheHubsKeys() throws {
    let v = try relayVector()
    var c = URLComponents(string: "switchboard://pair")!
    c.queryItems = [
        .init(name: "relay", value: "https://adarshnb.com/relay"), .init(name: "k", value: v["hub_kx_public"] as? String),
        .init(name: "s", value: v["sign_public"] as? String), .init(name: "code", value: "c0de"), .init(name: "hub", value: "h1"),
    ]
    let link = PairingLink(c.url!.absoluteString)
    #expect(link?.relay?.relayId == v["relay_id"] as? String)
    #expect(link?.hubURL.absoluteString == "https://adarshnb.com/relay")
    c.queryItems = c.queryItems!.filter { $0.name != "k" }
    #expect(PairingLink(c.url!.absoluteString) == nil, "a relay link without the hub's key is refused")
}

/// Plays relay and hub at once: opens the phone's envelope with the hub key and answers it.
final class MockRelay: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var hub = P256.KeyAgreement.PrivateKey()
    nonisolated(unsafe) static var status = 200
    /// A non-JSON error page, as nginx sends when the relay is down.
    nonisolated(unsafe) static var errorPage: Data? = nil
    nonisolated(unsafe) static var seen: [RelaySeal.PlainRequest] = []
    nonisolated(unsafe) static var paths: [String] = []

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}

    override func startLoading() {
        Self.paths.append(request.url!.path)
        var body = request.httpBody ?? Data()
        if body.isEmpty, let stream = request.httpBodyStream {
            stream.open()
            var buf = [UInt8](repeating: 0, count: 65536)
            while stream.hasBytesAvailable { let n = stream.read(&buf, maxLength: buf.count); if n <= 0 { break }; body.append(buf, count: n) }
        }
        let data: Data
        if Self.status != 200 {
            data = Self.errorPage ?? Data(#"{"error":{"code":"hub_offline","message":"x"}}"#.utf8)
        } else {
            let env = try! JSONDecoder().decode(RelaySeal.Envelope.self, from: body)
            let epk = try! P256.KeyAgreement.PublicKey(x963Representation: Data(base64URL: env.epk!)!)
            let shared = try! Self.hub.sharedSecretFromKeyAgreement(with: epk)
            let salt = epk.x963Representation + Self.hub.publicKey.x963Representation
            let key = { (info: String) in shared.hkdfDerivedSymmetricKey(using: SHA256.self, salt: salt, sharedInfo: Data(info.utf8), outputByteCount: 32) }
            let plain = try! RelaySeal.open(Data(base64URL: env.ct)!, key: key("switchboard-relay-v1 request"))
            Self.seen.append(try! JSONDecoder().decode(RelaySeal.PlainRequest.self, from: plain))
            let answer = RelaySeal.PlainResponse(status: 200, headers: [["content-type", "application/json"]],
                body: Data(#"{"api_version":1,"hub_id":"h1","min_client_build":1,"message_version":1,"max_wait_ms":25000}"#.utf8).base64EncodedString())
            let nonce = Data((0..<12).map { _ in UInt8.random(in: 0...255) })
            let ct = try! RelaySeal.seal(try! JSONEncoder().encode(answer), key: key("switchboard-relay-v1 reply"), nonce: nonce)
            data = try! JSONEncoder().encode(RelaySeal.Envelope(v: RelaySeal.version, epk: nil, n: nonce.base64URL, ct: ct.base64URL))
        }
        let resp = HTTPURLResponse(url: request.url!, statusCode: Self.status, httpVersion: nil, headerFields: ["content-type": "application/json"])!
        client?.urlProtocol(self, didReceive: resp, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: data)
        client?.urlProtocolDidFinishLoading(self)
    }
}

@Test(.serialized) func requestsGoThroughTheRelayEncryptedAndAnOfflineHubIsUnreachable() async throws {
    let config = URLSessionConfiguration.ephemeral
    config.protocolClasses = [MockRelay.self]
    let session = URLSession(configuration: config)
    let route = RelayRoute(relayURL: URL(string: "https://relay.test/relay")!,
                           hubKey: MockRelay.hub.publicKey.x963Representation.base64URL,
                           signKey: P256.Signing.PrivateKey().publicKey.x963Representation.base64URL)!
    let hub = Hub(pairing: Pairing(hubURL: route.relayURL, hubId: "h1", deviceId: "d1", name: "iPhone", relay: route),
                  keys: SoftwareKeys(), build: 3, transport: RelayTransport(route: route, session: session))
    let meta = try await hub.meta()
    #expect(meta.hubId == "h1")
    #expect(MockRelay.paths.last == "/relay/h/\(route.relayId)")
    let seen = try #require(MockRelay.seen.last)
    #expect(seen.method == "GET" && seen.path == "/app/v1/meta")
    #expect(seen.headers.contains(["x-switchboard-build", "3"]))

    MockRelay.status = 503
    defer { MockRelay.status = 200 }
    do {
        _ = try await hub.meta()
        Issue.record("expected an error")
    } catch let e as HubError {
        if case .unreachable = e {} else { Issue.record("an offline hub should be unreachable, got \(e)") }
    }

    MockRelay.status = 502
    MockRelay.errorPage = Data("<html>502 Bad Gateway</html>".utf8)
    defer { MockRelay.errorPage = nil }
    do {
        _ = try await hub.meta()
        Issue.record("expected an error")
    } catch let e as HubError {
        if case .unreachable = e {} else { Issue.record("a relay that's down should be unreachable, got \(e)") }
    }
}
