import Foundation
import Observation
import SwitchboardKit
import UIKit

/// The app's state: the pairing, the live session list and what the user is doing.
@MainActor
@Observable
final class AppModel {
    enum Phase: Equatable {
        case unpaired
        /// Waiting for the admin to confirm the fingerprint in the terminal.
        case confirming(fingerprint: String)
        case paired
    }

    let keys: DeviceKeys
    let build: Int
    private(set) var phase: Phase = .unpaired
    private(set) var pairing: Pairing?
    private(set) var hub: Hub?
    private(set) var sessions: [Schemas.Session] = []
    private(set) var connected = false
    private(set) var meta: Schemas.Meta?
    var pairingError: String?
    var banner: String?
    /// Set from a notification tap; the sessions screen opens it.
    var openSessionId: String?
    /// Where text in the message box that names no project goes.
    var current: Schemas.TargetView?
    /// Herdr agents Switchboard didn't start, refreshed while a list is on screen.
    private(set) var agents: [Schemas.AgentView] = []
    /// Those that ended in the last day.
    private(set) var ended: [Schemas.EndedAgent] = []
    private(set) var projects: [Schemas.Project] = []
    /// Bumped each time the app comes back to the foreground; screens restart their polls on it,
    /// since a long-poll begun before the phone slept may be on a dead connection.
    private(set) var wakes = 0
    /// What this phone last said to each session or agent, by its id. Kept on the phone only:
    /// the hub doesn't store prompts.
    private(set) var said: [String: Said] = [:]
    /// The last message sent from the home screen's voice or keyboard, shown there for a while.
    var lastSent: Sent?
    private var pollTask: Task<Void, Never>?
    private var pendingPushToken: Data?
    /// The push token the hub has; resent until it does.
    private var sentPushToken: Data?

    private static let pairingKey = "pairing"
    private static let saidStore = "said"
    private static let maxSaid = 50

    struct Said: Codable, Equatable {
        let text: String
        let at: Date
    }

    struct Sent: Equatable {
        let target: Schemas.TargetView?
        let text: String
        let started: Bool
        let at: Date
    }

    init(keys: DeviceKeys = SecureEnclaveKeys()) {
        self.keys = keys
        build = Int(Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "") ?? 1
        if ProcessInfo.processInfo.environment["SB_RESET"] == "1" {
            forget()
        }
        if let data = UserDefaults.standard.data(forKey: Self.saidStore),
           let s = try? JSONDecoder().decode([String: Said].self, from: data) {
            said = s
        }
        if let data = UserDefaults.standard.data(forKey: Self.pairingKey),
           let p = try? JSONDecoder().decode(Pairing.self, from: data) {
            use(p)
        }
    }

    private func use(_ p: Pairing) {
        pairing = p
        hub = Hub(pairing: p, keys: keys, build: build)
        phase = .paired
        startPolling()
        Task { await registerPush() }
    }

    // Pairing.

    /// Checks the link's hub, claims the code with fresh keys, then waits for the admin's yes.
    func pair(link: PairingLink, name: String) async {
        pairingError = nil
        let open = Hub(link: link, keys: keys, build: build)
        do {
            let meta = try await open.meta()
            guard meta.hubId == link.hubId else {
                pairingError = "This code is for a different hub than the one at \(link.hubURL.host() ?? "that address")."
                return
            }
            guard meta.minClientBuild <= build else {
                pairingError = "This hub needs a newer version of the app."
                return
            }
            try keys.reset()
            let claimed = try await open.claim(code: link.code, name: name, build: build)
            // The hub's fingerprint must be the one our own keys give, or someone swapped them.
            guard try open.localFingerprint(hubId: claimed.hubId) == claimed.fingerprint else {
                pairingError = "The hub reported keys that aren't this phone's. Don't pair; tell the admin."
                return
            }
            phase = .confirming(fingerprint: claimed.fingerprint)
            var state = claimed.state
            // The code lives ten minutes; Cancel ends the wait sooner.
            let deadline = Date().addingTimeInterval(600)
            while state == "pending", Date() < deadline {
                state = try await open.pairState(deviceId: claimed.deviceId)
                if Task.isCancelled { phase = .unpaired; return }
            }
            if state == "pending" { state = "not confirmed in time" }
            guard state == "active" else {
                phase = .unpaired
                pairingError = "Pairing was \(state) at the hub. Ask for a new code."
                return
            }
            let p = Pairing(hubURL: link.hubURL, hubId: claimed.hubId, deviceId: claimed.deviceId, name: name, relay: link.relay)
            UserDefaults.standard.set(try JSONEncoder().encode(p), forKey: Self.pairingKey)
            use(p)
        } catch {
            phase = .unpaired
            if !Task.isCancelled { pairingError = error.localizedDescription }
        }
    }

    /// Forgets the hub at the user's request, first asking it to stop pushing to this phone.
    func unpair() async {
        if let hub { try? await hub.setPushToken(nil, sandbox: sandbox) }
        forget()
    }

    /// Forgets the hub and this phone's keys. The hub keeps the device until it's revoked.
    func forget() {
        pollTask?.cancel()
        UserDefaults.standard.removeObject(forKey: Self.pairingKey)
        UserDefaults.standard.removeObject(forKey: Self.saidStore)
        said = [:]
        lastSent = nil
        try? keys.reset()
        pairing = nil
        hub = nil
        sessions = []
        agents = []
        ended = []
        projects = []
        current = nil
        meta = nil
        connected = false
        sentPushToken = nil
        phase = .unpaired
    }

    /// The app is in the foreground again: restart every poll and resend anything still owed.
    func becameActive() {
        guard phase == .paired else { return }
        wakes += 1
        startPolling()
    }

    /// Remembers a message this phone sent, for the session page and the home screen.
    func record(sent text: String, to target: Schemas.TargetView?, started: Bool) {
        lastSent = Sent(target: target, text: text, started: started, at: Date())
        guard let key = target.map(Self.saidKey(for:)) else { return }
        said[key] = Said(text: text, at: Date())
        if said.count > Self.maxSaid, let oldest = said.min(by: { $0.value.at < $1.value.at })?.key {
            said[oldest] = nil
        }
        UserDefaults.standard.set(try? JSONEncoder().encode(said), forKey: Self.saidStore)
    }

    static func saidKey(for t: Schemas.TargetView) -> String {
        t.sessionId ?? "agent:\(t.machineId)/\(t.agent ?? t.label)"
    }

    /// Names the speech recognizer should expect: projects, sessions and agents, as written
    /// and as spoken.
    var vocabulary: [String] {
        var names = ["Switchboard", "Herdr", "Claude", "Codex", "worktree"]
        for p in projects { names += [p.id] + p.aliases }
        for s in sessions { names += [s.label, s.branch].compactMap { $0 } }
        for a in agents where a.displayName == a.name { names.append(a.name.split(separator: "/").last.map(String.init) ?? a.name) }
        var seen = Set<String>()
        var out: [String] = []
        for name in names {
            let spoken = name.replacing(/[-_\/]+/, with: " ")
            for w in [name, spoken] where seen.insert(w.lowercased()).inserted { out.append(w) }
        }
        // Apple suggests no more than about 100 phrases.
        return Array(out.prefix(100))
    }

    func refreshAgents() async {
        guard let hub, let list = try? await hub.agents() else { return }
        agents = list.agents
        ended = list.ended ?? []
    }

    /// Every Herdr session with its agents, whoever started them. Ended agents count from `since`,
    /// or all that are known when it is nil.
    func herdrSessions(machine: String? = nil, since: Int64?) -> [HerdrSession] {
        HerdrSession.group(sessions: sessions, agents: agents, ended: ended, machine: machine, since: since)
    }

    // The session list, long-polled: the hub answers when something newer than our revision exists.

    func startPolling() {
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            var revision: Int64?
            while !Task.isCancelled {
                guard let self, let hub = self.hub else { return }
                do {
                    if self.meta == nil { self.meta = try await hub.meta() }
                    if self.projects.isEmpty, let p = try? await hub.projects() { self.projects = p }
                    let list = try await hub.sessions(after: revision)
                    self.sessions = list.sessions
                    revision = list.revision
                    self.connected = true
                    if let token = self.pendingPushToken, token != self.sentPushToken {
                        await self.sendPushToken(token)
                    }
                } catch {
                    self.connected = false
                    if self.handleRevoked(error) { return }
                    try? await Task.sleep(for: .seconds(3))
                }
            }
        }
    }

    /// A refused token refresh means the hub no longer knows this device.
    func handleRevoked(_ error: Error) -> Bool {
        guard let e = error as? HubError, e.code == "unknown_device" else { return false }
        forget()
        pairingError = "This phone was revoked at the hub. Pair it again."
        return true
    }

    // Push.

    func registerPush() async {
        let center = UNUserNotificationCenter.current()
        guard (try? await center.requestAuthorization(options: [.alert, .sound])) == true else { return }
        UIApplication.shared.registerForRemoteNotifications()
        if let token = pendingPushToken { await sendPushToken(token) }
    }

    func receivedPushToken(_ token: Data) async {
        pendingPushToken = token
        await sendPushToken(token)
    }

    private func sendPushToken(_ token: Data) async {
        guard let hub else { return }
        if (try? await hub.setPushToken(token, sandbox: sandbox)) != nil { sentPushToken = token }
    }

    /// From how this build was signed, not Debug or Release: a Release build installed from
    /// Xcode still gets development-environment tokens.
    private var sandbox: Bool {
        let profile = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision")
            .flatMap { try? Data(contentsOf: $0) }
        #if targetEnvironment(simulator)
        return APNsEnvironment.isSandbox(profile: profile, simulator: true)
        #else
        return APNsEnvironment.isSandbox(profile: profile, simulator: false)
        #endif
    }
}
