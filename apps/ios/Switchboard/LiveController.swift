import ActivityKit
import Foundation
import SwitchboardKit

/// Keeps the lock-screen card. While the app runs it updates the card itself from the hub's
/// board; while it doesn't, the hub pushes sealed boards to the tokens this registers. A card
/// is ended and replaced before iOS's eight-hour limit.
@MainActor
final class LiveController {
    /// Replaced a little before iOS ends a card at eight hours.
    static let maxAge: TimeInterval = 7 * 3600
    private var hub: Hub?
    private var startToken: Data?
    private var sent: (key: Data, start: Data?, activity: Data?, at: Date?, mode: LiveShowMode)?
    /// When the board went quiet, for "while working"'s fifteen-minute end.
    private var quietSince: Date?
    private var shown: LiveBoard?
    private var observing = false

    private var activity: Activity<AgentsActivity>? { Activity<AgentsActivity>.activities.first }

    /// Starts following token changes; called at launch, since iOS may start a card through
    /// the hub's push and wake the app only to learn its token.
    func observe() {
        guard !observing else { return }
        observing = true
        if #available(iOS 17.2, *) {
            Task { [weak self] in
                for await token in Activity<AgentsActivity>.pushToStartTokenUpdates {
                    self?.startToken = token
                    await self?.register()
                }
            }
        }
        Task { [weak self] in
            for await a in Activity<AgentsActivity>.activityUpdates {
                self?.follow(a)
            }
        }
        for a in Activity<AgentsActivity>.activities { follow(a) }
    }

    private func follow(_ a: Activity<AgentsActivity>) {
        Task { [weak self] in
            for await _ in a.pushTokenUpdates { await self?.register() }
        }
        Task { [weak self] in
            for await state in a.activityStateUpdates where state == .ended || state == .dismissed {
                await self?.register()
            }
        }
    }

    func use(_ hub: Hub?) {
        self.hub = hub
        sent = nil
        Task { await register() }
    }

    /// Sends the registration again, as a restarted or updated hub may not have all of it.
    func resend() async {
        sent = nil
        await register()
    }

    /// Tells the hub where the card is; skipped when nothing changed.
    func register() async {
        guard let hub, ActivityAuthorizationInfo().areActivitiesEnabled,
              let key = try? LiveKey.shared.loadOrCreate().publicKey.x963Representation else { return }
        let live = activity.flatMap { $0.activityState == .active ? $0 : nil }
        let now = (key: key, start: startToken, activity: live?.pushToken, at: live.flatMap { Self.started[$0.id] }, mode: LiveShowMode.current)
        if let sent, sent.key == now.key, sent.start == now.start, sent.activity == now.activity, sent.at == now.at, sent.mode == now.mode { return }
        if (try? await hub.setLive(key: now.key, startToken: now.start, activityToken: now.activity, startedAt: now.at, mode: now.mode)) != nil {
            sent = now
        }
    }

    /// When each card started, by activity id; a card the hub started counts from first sight.
    private static var started: [String: Date] {
        get {
            let raw = UserDefaults.standard.dictionary(forKey: "liveStarted") as? [String: Double] ?? [:]
            return raw.mapValues { Date(timeIntervalSince1970: $0) }
        }
        set {
            let live = Set(Activity<AgentsActivity>.activities.map(\.id))
            let kept = newValue.filter { live.contains($0.key) }.mapValues(\.timeIntervalSince1970)
            UserDefaults.standard.set(kept, forKey: "liveStarted")
        }
    }

    /// Reads the board and shows it; called while the app is in the foreground.
    func refresh() async {
        #if DEBUG
        // A demo card stays as the demo set it.
        if ProcessInfo.processInfo.environment["SB_LIVE_DEMO"] != nil { return }
        #endif
        guard let hub, ActivityAuthorizationInfo().areActivitiesEnabled else { return }
        do {
            await show(try await hub.live())
        } catch let e as HubError {
            // A hub that answers without a board (an older one, or an error) leaves no card up:
            // the card only ever shows what the hub sent. One that can't be reached keeps it,
            // and it goes stale on its own.
            if case .unreachable = e { return }
            await Self.end(nil, nil)
            shown = nil
        } catch {
            await Self.end(nil, nil)
            shown = nil
        }
    }

    func show(_ board: LiveBoard) async {
        guard let key = try? LiveKey.shared.loadOrCreate(),
              let blob = try? LiveSeal.seal(JSONEncoder().encode(board), to: key.publicKey) else { return }
        let content = ActivityContent(state: AgentsActivity.ContentState(b: blob), staleDate: Date().addingTimeInterval(3600))
        quietSince = board.isQuiet ? (quietSince ?? Date()) : nil
        let live = activity.flatMap { $0.activityState == .active ? $0 : nil }
        let step = LiveShowMode.current.decide(board, cardShowing: live != nil, quietSince: quietSince)
        if let a = live {
            let id = a.id
            if let at = Self.started[id], Date().timeIntervalSince(at) > Self.maxAge {
                await Self.end(id, nil)
            } else {
                if step == .end {
                    await Self.end(id, content)
                    shown = nil
                } else if shown.map({ !$0.sameAs(board) }) ?? true {
                    await Self.update(id, content)
                    shown = board
                }
                Self.started[id] = Self.started[id] ?? Date()
                return
            }
        }
        guard step == .show else { return }
        do {
            let a = try Activity.request(attributes: AgentsActivity(), content: content, pushType: .token)
            Self.started[a.id] = Date()
            shown = board
            follow(a)
            await register()
        } catch {
            NSLog("Couldn't start the lock-screen card: %@", String(describing: error))
        }
    }

    // Activities aren't Sendable, so each change finds its card by id off the main actor.
    nonisolated private static func update(_ id: String, _ content: ActivityContent<AgentsActivity.ContentState>) async {
        for a in Activity<AgentsActivity>.activities where a.id == id { await a.update(content) }
    }

    nonisolated private static func end(_ id: String?, _ content: ActivityContent<AgentsActivity.ContentState>?) async {
        for a in Activity<AgentsActivity>.activities where id == nil || a.id == id {
            await a.end(content, dismissalPolicy: .immediate)
        }
    }

    /// Ends the card, e.g. when forgetting the hub.
    func end() async {
        await Self.end(nil, nil)
        try? await hub?.clearLive()
        shown = nil
        sent = nil
    }
}
