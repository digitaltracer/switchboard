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
    private var pollTask: Task<Void, Never>?
    private var pendingPushToken: Data?

    private static let pairingKey = "pairing"

    init(keys: DeviceKeys = SecureEnclaveKeys()) {
        self.keys = keys
        build = Int(Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "") ?? 1
        if ProcessInfo.processInfo.environment["SB_RESET"] == "1" {
            forget()
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
        let open = Hub(url: link.hubURL, keys: keys, build: build)
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
            while state == "pending" {
                state = try await open.pairState(deviceId: claimed.deviceId)
            }
            guard state == "active" else {
                phase = .unpaired
                pairingError = "Pairing was \(state) at the hub. Ask for a new code."
                return
            }
            let p = Pairing(hubURL: link.hubURL, hubId: claimed.hubId, deviceId: claimed.deviceId, name: name)
            UserDefaults.standard.set(try JSONEncoder().encode(p), forKey: Self.pairingKey)
            use(p)
        } catch {
            phase = .unpaired
            pairingError = error.localizedDescription
        }
    }

    /// Forgets the hub and this phone's keys. The hub keeps the device until it's revoked.
    func forget() {
        pollTask?.cancel()
        UserDefaults.standard.removeObject(forKey: Self.pairingKey)
        try? keys.reset()
        pairing = nil
        hub = nil
        sessions = []
        phase = .unpaired
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
                    let list = try await hub.sessions(after: revision)
                    self.sessions = list.sessions
                    revision = list.revision
                    self.connected = true
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
        guard (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) == true else { return }
        UIApplication.shared.registerForRemoteNotifications()
        if let token = pendingPushToken { await sendPushToken(token) }
    }

    func receivedPushToken(_ token: Data) async {
        pendingPushToken = token
        await sendPushToken(token)
    }

    private func sendPushToken(_ token: Data) async {
        guard let hub else { return }
        #if DEBUG
        let sandbox = true
        #else
        let sandbox = false
        #endif
        try? await hub.setPushToken(token, sandbox: sandbox)
    }
}
