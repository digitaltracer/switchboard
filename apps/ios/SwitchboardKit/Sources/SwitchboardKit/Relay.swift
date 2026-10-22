import CryptoKit
import Foundation
import HTTPTypes
import OpenAPIRuntime

/// How a paired phone reaches a hub through a relay. The relay id is a hash of the hub's two
/// public keys, so the relay can't send this phone's traffic to a hub of its own choosing.
public struct RelayRoute: Codable, Equatable, Sendable {
    public var relayURL: URL
    /// The hub's key-agreement key, SEC1 uncompressed, base64url.
    public var hubKey: String
    public var signKey: String
    public var relayId: String

    public init?(relayURL: URL, hubKey: String, signKey: String) {
        guard let kx = Data(base64URL: hubKey), let sign = Data(base64URL: signKey),
              (try? P256.KeyAgreement.PublicKey(x963Representation: kx)) != nil else { return nil }
        self.relayURL = relayURL.withoutTrailingSlash
        self.hubKey = hubKey
        self.signKey = signKey
        relayId = SHA256.hash(data: kx + sign).map { String(format: "%02x", $0) }.joined().prefix(32).description
    }
}

/// End-to-end encryption to the hub, as `src/relay/seal.rs` does it: ECDH with a one-off key,
/// HKDF-SHA256 into a request key and a reply key, ChaCha20-Poly1305. Requests use a zero nonce
/// under their single-use key; each reply carries its own random nonce.
enum RelaySeal {
    static let version = 2

    struct Envelope: Codable {
        var v: Int
        var epk: String?
        var n: String?
        var ct: String
    }

    struct PlainRequest: Codable {
        var ts: Int64
        var method: String
        var path: String
        var headers: [[String]]
        var body: String
    }

    struct PlainResponse: Codable {
        var status: Int
        var headers: [[String]]
        var body: String
    }

    static func keys(eph: P256.KeyAgreement.PrivateKey, hub: P256.KeyAgreement.PublicKey) throws -> (request: SymmetricKey, reply: SymmetricKey) {
        let shared = try eph.sharedSecretFromKeyAgreement(with: hub)
        let salt = eph.publicKey.x963Representation + hub.x963Representation
        let derive = { (info: String) in
            shared.hkdfDerivedSymmetricKey(using: SHA256.self, salt: salt, sharedInfo: Data(info.utf8), outputByteCount: 32)
        }
        return (derive("switchboard-relay-v1 request"), derive("switchboard-relay-v1 reply"))
    }

    static func seal(_ plain: Data, key: SymmetricKey, nonce: Data = Data(count: 12)) throws -> Data {
        let box = try ChaChaPoly.seal(plain, using: key, nonce: try ChaChaPoly.Nonce(data: nonce))
        return box.ciphertext + box.tag
    }

    static func open(_ ct: Data, key: SymmetricKey, nonce: Data = Data(count: 12)) throws -> Data {
        guard ct.count >= 16 else { throw HubError.unexpected("The hub's answer is too short.") }
        let box = try ChaChaPoly.SealedBox(
            nonce: try ChaChaPoly.Nonce(data: nonce), ciphertext: ct.dropLast(16), tag: ct.suffix(16)
        )
        return try ChaChaPoly.open(box, using: key)
    }
}

/// Sends each API request through the relay, encrypted to the hub. The generated client and
/// everything above it don't know the difference.
public struct RelayTransport: ClientTransport {
    let route: RelayRoute
    let session: URLSession

    public init(route: RelayRoute, session: URLSession = .shared) {
        self.route = route
        self.session = session
    }

    public func send(_ request: HTTPRequest, body: HTTPBody?, baseURL: URL, operationID: String) async throws -> (HTTPResponse, HTTPBody?) {
        guard let kx = Data(base64URL: route.hubKey) else { throw HubError.unexpected("Bad hub key.") }
        let hub = try P256.KeyAgreement.PublicKey(x963Representation: kx)
        var bodyData = Data()
        if let body { bodyData = try await Data(collecting: body, upTo: 2 << 20) }
        let plain = RelaySeal.PlainRequest(
            ts: Countdown.nowMs(),
            method: request.method.rawValue,
            path: request.path ?? "/",
            headers: request.headerFields.map { [$0.name.rawName, $0.value] },
            body: bodyData.base64EncodedString()
        )
        let eph = P256.KeyAgreement.PrivateKey()
        let keys = try RelaySeal.keys(eph: eph, hub: hub)
        let envelope = RelaySeal.Envelope(
            v: RelaySeal.version,
            epk: eph.publicKey.x963Representation.base64URL,
            n: nil,
            ct: try RelaySeal.seal(try JSONEncoder().encode(plain), key: keys.request).base64URL
        )

        var post = URLRequest(url: route.relayURL.appending(path: "h/\(route.relayId)"))
        post.httpMethod = "POST"
        post.setValue("application/json", forHTTPHeaderField: "content-type")
        post.httpBody = try JSONEncoder().encode(envelope)
        // The relay answers within 45 s, with the hub's reply or its own timeout.
        post.timeoutInterval = 50
        let (data, response) = try await session.data(for: post)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard status == 200 else {
            let decoded = try? JSONDecoder().decode(Schemas.ApiError.self, from: data)
            let code = decoded?.error.code ?? "relay_\(status)"
            // An offline or slow hub, or a relay that's down, is "can't reach", so a start goes to
            // reconciling, not failed.
            if ["hub_offline", "hub_timeout"].contains(code) {
                throw HubError.unreachable(code == "hub_offline" ? "Your hub isn't connected to the relay." : "Your hub didn't answer in time.")
            }
            if decoded == nil, status >= 500 || status == 0 {
                throw HubError.unreachable("The relay isn't answering.")
            }
            throw HubError.hub(code: code, message: decoded?.error.message ?? "The relay answered \(status).", operationId: nil, sessionId: nil)
        }
        let reply = try JSONDecoder().decode(RelaySeal.Envelope.self, from: data)
        guard reply.v == RelaySeal.version, let ct = Data(base64URL: reply.ct),
              let nonce = reply.n.flatMap({ Data(base64URL: $0) }), nonce.count == 12
        else { throw HubError.unexpected("Unexpected relay answer.") }
        let answer = try JSONDecoder().decode(RelaySeal.PlainResponse.self, from: try RelaySeal.open(ct, key: keys.reply, nonce: nonce))
        var out = HTTPResponse(status: .init(code: answer.status))
        for pair in answer.headers where pair.count == 2 {
            if let name = HTTPField.Name(pair[0]) { out.headerFields.append(HTTPField(name: name, value: pair[1])) }
        }
        return (out, HTTPBody(Data(base64Encoded: answer.body) ?? Data()))
    }
}
