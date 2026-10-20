import Foundation
import HTTPTypes
import OpenAPIRuntime
import OpenAPIURLSession
import SwitchboardAPI

public typealias Schemas = Components.Schemas

/// An error the hub answered with, by its stable code, or a failure to reach it.
public enum HubError: Error, LocalizedError, Equatable {
    case hub(code: String, message: String, operationId: String?, sessionId: String?)
    /// No answer: the request may or may not have reached the hub.
    case unreachable(String)
    case notPaired
    case unexpected(String)

    public var code: String? {
        if case .hub(let code, _, _, _) = self { return code }
        return nil
    }

    public var errorDescription: String? {
        switch self {
        case .hub(_, let message, _, _): return message
        case .unreachable(let why): return "Can't reach the hub: \(why)"
        case .notPaired: return "This phone isn't paired."
        case .unexpected(let why): return why
        }
    }
}

/// What a paired phone remembers about its hub. Nothing here is secret; the keys are.
public struct Pairing: Codable, Equatable, Sendable {
    public var hubURL: URL
    public var hubId: String
    public var deviceId: String
    public var name: String

    public init(hubURL: URL, hubId: String, deviceId: String, name: String) {
        self.hubURL = hubURL
        self.hubId = hubId
        self.deviceId = deviceId
        self.name = name
    }
}

/// `switchboard://pair?url=…&code=…&hub=…`, from the QR code `switchboard admin pair-device` shows.
public struct PairingLink: Equatable, Sendable {
    public var hubURL: URL
    public var code: String
    public var hubId: String

    public init?(_ text: String) {
        guard let c = URLComponents(string: text.trimmingCharacters(in: .whitespacesAndNewlines)),
              c.scheme == "switchboard", c.host == "pair" else { return nil }
        let q = Dictionary((c.queryItems ?? []).compactMap { i in i.value.map { (i.name, $0) } }, uniquingKeysWith: { a, _ in a })
        guard let raw = q["url"], let url = URL(string: raw)?.withoutTrailingSlash, let scheme = url.scheme,
              ["https", "http"].contains(scheme), let code = q["code"], !code.isEmpty,
              let hub = q["hub"], !hub.isEmpty else { return nil }
        hubURL = url
        self.code = code
        hubId = hub
    }
}

extension URL {
    /// The generated client appends `/app/v1/…`, so a base URL ending in `/` would give `//app`.
    var withoutTrailingSlash: URL {
        var s = absoluteString
        while s.hasSuffix("/") { s.removeLast() }
        return URL(string: s) ?? self
    }
}

/// Operations that work without a device token.
private let openOperations: Set<String> = ["get_meta", "pair", "pair_status", "token_challenge", "issue_token"]

/// Turns the hub's JSON errors into `HubError` and adds the build header, before the
/// generated client looks at the response.
struct HubMiddleware: ClientMiddleware {
    let build: Int
    let tokens: TokenSource?

    func intercept(
        _ request: HTTPRequest,
        body: HTTPBody?,
        baseURL: URL,
        operationID: String,
        next: @Sendable (HTTPRequest, HTTPBody?, URL) async throws -> (HTTPResponse, HTTPBody?)
    ) async throws -> (HTTPResponse, HTTPBody?) {
        var request = request
        request.headerFields[HTTPField.Name("x-switchboard-build")!] = String(build)
        let authed = !openOperations.contains(operationID)
        if authed {
            guard let tokens else { throw HubError.notPaired }
            request.headerFields[.authorization] = "Bearer \(try await tokens.token())"
        }
        var (response, responseBody) = try await next(request, body, baseURL)
        if authed, response.status.code == 401, let tokens {
            // An expired or revoked token: try once with a fresh one.
            request.headerFields[.authorization] = "Bearer \(try await tokens.token(forceRefresh: true))"
            (response, responseBody) = try await next(request, body, baseURL)
        }
        guard response.status.code >= 400 else { return (response, responseBody) }
        var data = Data()
        if let responseBody { data = try await Data(collecting: responseBody, upTo: 1 << 20) }
        let decoded = try? JSONDecoder().decode(Schemas.ApiError.self, from: data)
        throw HubError.hub(
            code: decoded?.error.code ?? "http_\(response.status.code)",
            message: decoded?.error.message ?? "The hub answered \(response.status.code).",
            operationId: decoded?.error.operationId,
            sessionId: decoded?.error.sessionId
        )
    }
}

/// Short-lived access tokens, refreshed by signing a challenge with the auth key.
actor TokenSource {
    private let client: Client
    private let deviceId: String
    private let keys: DeviceKeys
    private var current: (token: String, expiresAt: Int64)?

    init(client: Client, deviceId: String, keys: DeviceKeys) {
        self.client = client
        self.deviceId = deviceId
        self.keys = keys
    }

    func token(forceRefresh: Bool = false) async throws -> String {
        let soon = Int64(Date().timeIntervalSince1970 * 1000) + 60_000
        if !forceRefresh, let c = current, c.expiresAt > soon { return c.token }
        let ch = try await Hub.unwrap { try await client.tokenChallenge(body: .json(.init(deviceId: deviceId))).ok.body.json }
        let message = SignedMessage.token(hub: ch.hubId, device: deviceId, nonce: ch.nonce, expiresAt: ch.expiresAt)
        let signature = try await keys.sign(message, with: .auth, reason: "Sign in to Switchboard")
        let t = try await Hub.unwrap {
            try await client.issueToken(body: .json(.init(
                deviceId: deviceId, nonce: ch.nonce, signature: signature.base64URL, messageVersion: SignedMessage.version
            ))).ok.body.json
        }
        current = (t.accessToken, t.expiresAt)
        return t.accessToken
    }
}

/// The app's view of one hub. Every call re-checks with the hub; nothing is trusted from a push.
public final class Hub: Sendable {
    public let pairing: Pairing?
    public let baseURL: URL
    private let client: Client
    private let keys: DeviceKeys

    /// A hub the phone hasn't paired with yet: only meta and pairing calls work.
    public convenience init(url: URL, keys: DeviceKeys, build: Int, transport: any ClientTransport = URLSessionTransport()) {
        self.init(url: url, pairing: nil, keys: keys, build: build, transport: transport)
    }

    public convenience init(pairing: Pairing, keys: DeviceKeys, build: Int, transport: any ClientTransport = URLSessionTransport()) {
        self.init(url: pairing.hubURL, pairing: pairing, keys: keys, build: build, transport: transport)
    }

    private init(url: URL, pairing: Pairing?, keys: DeviceKeys, build: Int, transport: any ClientTransport) {
        let url = url.withoutTrailingSlash
        self.pairing = pairing
        self.baseURL = url
        self.keys = keys
        let open = Client(serverURL: url, transport: transport, middlewares: [HubMiddleware(build: build, tokens: nil)])
        let tokens = pairing.map { TokenSource(client: open, deviceId: $0.deviceId, keys: keys) }
        client = Client(serverURL: url, transport: transport, middlewares: [HubMiddleware(build: build, tokens: tokens)])
    }

    /// Runs a generated call, surfacing the hub's own error instead of the client's wrapper.
    static func unwrap<T: Sendable>(_ call: @Sendable () async throws -> T) async throws -> T {
        do {
            return try await call()
        } catch let e as ClientError {
            if let hub = e.underlyingError as? HubError { throw hub }
            // No HTTP response at all: the request may or may not have reached the hub.
            if e.response == nil { throw HubError.unreachable(e.underlyingError.localizedDescription) }
            throw HubError.unexpected(String(describing: e.underlyingError))
        }
    }

    // Pairing.

    public func meta() async throws -> Schemas.Meta {
        try await Self.unwrap { try await client.getMeta().ok.body.json }
    }

    /// Claims the code with this phone's public keys. Show the returned fingerprint.
    public func claim(code: String, name: String, build: Int) async throws -> Schemas.PairResponse {
        let auth = try keys.publicKey(.auth).base64URL
        let approval = try keys.publicKey(.approval).base64URL
        return try await Self.unwrap {
            try await client.pair(body: .json(.init(
                code: code, name: name, authKey: auth, approvalKey: approval, clientBuild: Int64(build)
            ))).ok.body.json
        }
    }

    /// Waits up to 25 s for the admin to confirm; returns the device's state.
    public func pairState(deviceId: String) async throws -> String {
        try await Self.unwrap {
            try await client.pairStatus(path: .init(deviceId: deviceId), query: .init(waitMs: 25_000)).ok.body.json.state
        }
    }

    /// The fingerprint this phone's keys give for `hubId`, to compare with the hub's.
    public func localFingerprint(hubId: String) throws -> String {
        pairingFingerprint(hubId: hubId, authKey: try keys.publicKey(.auth).base64URL, approvalKey: try keys.publicKey(.approval).base64URL)
    }

    // Sessions.

    public func projects() async throws -> [Schemas.Project] {
        try await Self.unwrap { try await client.listProjects().ok.body.json }
    }

    public func machines() async throws -> [Schemas.Machine] {
        try await Self.unwrap { try await client.listMachines().ok.body.json }
    }

    /// With `after`, waits up to `waitMs` for a list newer than that revision.
    public func sessions(after: Int64? = nil, waitMs: Int64 = 25_000) async throws -> Schemas.Sessions {
        try await Self.unwrap {
            try await client.listSessions(query: .init(afterRevision: after, waitMs: after == nil ? nil : waitMs)).ok.body.json
        }
    }

    public func session(_ id: String, after: Int64? = nil, waitMs: Int64 = 25_000) async throws -> Schemas.Session {
        try await Self.unwrap {
            try await client.getSession(path: .init(id: id), query: .init(afterRevision: after, waitMs: after == nil ? nil : waitMs)).ok.body.json
        }
    }

    public func output(_ id: String, lines: Int = 80) async throws -> Schemas.Output {
        try await Self.unwrap { try await client.getOutput(path: .init(id: id), query: .init(lines: Int32(lines))).ok.body.json }
    }

    public func start(project: String, prompt: String, machine: String?, agent: String?, notify: Bool, requestId: String) async throws -> Schemas.StartResponse {
        try await Self.unwrap {
            try await client.startSession(body: .json(.init(
                projectId: project, prompt: prompt, machineId: machine, agent: agent, notify: notify, requestId: requestId
            ))).ok.body.json
        }
    }

    public func operation(_ requestId: String) async throws -> Schemas.OperationStatus {
        try await Self.unwrap { try await client.getOperation(path: .init(requestId: requestId)).ok.body.json }
    }

    public func prompt(_ id: String, text: String, requestId: String) async throws -> Schemas.PromptResponse {
        try await Self.unwrap {
            try await client.promptSession(path: .init(id: id), body: .json(.init(text: text, requestId: requestId))).ok.body.json
        }
    }

    public func stop(_ id: String) async throws -> Schemas.StopResponse {
        try await Self.unwrap {
            try await client.stopSession(path: .init(id: id), body: .json(.init(removeWorktreeIfClean: false))).ok.body.json
        }
    }

    public func mute(_ id: String, muted: Bool) async throws -> Schemas.MuteResponse {
        try await Self.unwrap { try await client.muteSession(path: .init(id: id), body: .json(.init(muted: muted))).ok.body.json }
    }

    // Approvals.

    public func deny(_ approvalId: String, requestId: String) async throws -> Schemas.DecisionResponse {
        try await Self.unwrap {
            try await client.decideApproval(path: .init(id: approvalId), body: .json(.init(decision: .deny, requestId: requestId))).ok.body.json
        }
    }

    /// Signs a fresh challenge with the approval key, which asks for Face ID or the passcode.
    public func approve(_ approvalId: String, reason: String, requestId: String) async throws -> Schemas.DecisionResponse {
        guard let pairing else { throw HubError.notPaired }
        let ch = try await Self.unwrap { try await client.approvalChallenge(path: .init(id: approvalId)).ok.body.json }
        let message = SignedMessage.approval(hub: ch.hubId, device: pairing.deviceId, approval: approvalId, nonce: ch.nonce, expiresAt: ch.expiresAt)
        let signature = try await keys.sign(message, with: .approval, reason: reason)
        return try await Self.unwrap {
            try await client.decideApproval(path: .init(id: approvalId), body: .json(.init(
                decision: .allow, nonce: ch.nonce, signature: signature.base64URL, messageVersion: SignedMessage.version, requestId: requestId
            ))).ok.body.json
        }
    }

    // Devices.

    public func devices() async throws -> [Schemas.Device] {
        try await Self.unwrap { try await client.listDevices().ok.body.json }
    }

    public func revoke(_ deviceId: String) async throws {
        _ = try await Self.unwrap { try await client.revokeDevice(path: .init(id: deviceId)).noContent }
    }

    public func setPushToken(_ token: Data?, sandbox: Bool) async throws {
        let hex = token.map { $0.map { String(format: "%02x", $0) }.joined() }
        _ = try await Self.unwrap {
            try await client.setPushToken(body: .json(.init(token: hex, environment: sandbox ? .sandbox : .production))).noContent
        }
    }
}
