import Foundation

/// A Herdr session and every agent in it, whoever started them: running ones first, then those
/// that ended.
public struct HerdrSession: Identifiable, Hashable, Sendable {
    public struct Key: Hashable, Sendable {
        public let machine: String
        public let name: String

        public init(machine: String, name: String) {
            self.machine = machine
            self.name = name
        }
    }

    public enum Agent: Hashable, Sendable {
        /// One Switchboard started, with its worktree and approvals.
        case task(Schemas.Session)
        case live(Schemas.AgentView)
        case ended(Schemas.EndedAgent)
    }

    public let key: Key
    public internal(set) var live: [Agent] = []
    public internal(set) var ended: [Agent] = []

    public init(key: Key) {
        self.key = key
    }
    public var id: Key { key }
    /// No agent is left running in it.
    public var isEnded: Bool { live.isEmpty }
    public var agents: [Agent] { live + ended }

    /// When its last agent ended, for a session that has ended.
    public var endedAt: Int64 { ended.map(\.endedAt).max() ?? 0 }

    /// Groups the hub's lists by machine and Herdr session. Ended agents count from `since`, or
    /// all of them when it is nil; `machine` keeps only that machine's.
    public static func group(sessions: [Schemas.Session], agents: [Schemas.AgentView],
                             ended: [Schemas.EndedAgent], machine: String? = nil,
                             since: Int64?) -> [HerdrSession] {
        var groups: [Key: HerdrSession] = [:]
        func add(_ a: Agent, machine m: String, session: String?, live: Bool) {
            guard machine == nil || machine == m else { return }
            let key = Key(machine: m, name: session ?? "default")
            if live { groups[key, default: .init(key: key)].live.append(a) } else { groups[key, default: .init(key: key)].ended.append(a) }
        }
        for a in agents { add(.live(a), machine: a.machineId, session: a.herdrSession, live: true) }
        for s in sessions.sorted(by: { $0.createdAt < $1.createdAt }) {
            let live = Self.liveLifecycles.contains(s.lifecycle)
            if !live, let since, s.updatedAt < since { continue }
            add(.task(s), machine: s.machineId, session: s.herdrSession, live: live)
        }
        for e in ended {
            if let since, e.endedAt < since { continue }
            add(.ended(e), machine: e.machineId, session: e.herdrSession, live: false)
        }
        let all = groups.values.map { g -> HerdrSession in
            var g = g
            g.ended.sort { $0.endedAt > $1.endedAt }
            return g
        }
        let running = all.filter { !$0.isEnded }.sorted { ($0.title.lowercased(), $0.key.machine) < ($1.title.lowercased(), $1.key.machine) }
        let done = all.filter(\.isEnded).sorted { $0.endedAt > $1.endedAt }
        return running + done
    }

    static let liveLifecycles: Set<String> = ["starting", "running", "stopping"]

    /// The name the hub gives the Herdr app's one session. The app has spaces, not sessions.
    public static let herdrApp = "hc"

    public var isHerdrApp: Bool { key.name == Self.herdrApp }

    /// What to call it on screen.
    public var title: String { Self.title(key.name) }

    public static func title(_ name: String) -> String { name == herdrApp ? "Herdr App" : name }

    /// One space of a session, with the agents in it. `name` is nil for agents in no named space.
    public struct Space: Identifiable, Hashable, Sendable {
        public let name: String?
        public internal(set) var live: [Agent] = []
        public internal(set) var ended: [Agent] = []
        public var id: String { name ?? "" }
        public var agents: [Agent] { live + ended }
    }

    /// Its agents by space: each named space in name order, then those in none. Within a space,
    /// running ones come before those that ended.
    public var spaces: [Space] {
        var named: [String: Space] = [:]
        var rest = Space(name: nil)
        func add(_ a: Agent, live: Bool) {
            if let name = a.space {
                if live { named[name, default: Space(name: name)].live.append(a) } else { named[name, default: Space(name: name)].ended.append(a) }
            } else if live {
                rest.live.append(a)
            } else {
                rest.ended.append(a)
            }
        }
        for a in live { add(a, live: true) }
        for a in ended { add(a, live: false) }
        let sorted = named.values.sorted { $0.id.localizedCaseInsensitiveCompare($1.id) == .orderedAscending }
        return rest.agents.isEmpty ? sorted : sorted + [rest]
    }

    /// Where today began on this phone, in the hub's milliseconds.
    public static func startOfToday(_ now: Date = Date()) -> Int64 {
        Int64(Calendar.current.startOfDay(for: now).timeIntervalSince1970 * 1000)
    }
}

extension HerdrSession.Agent {
    public var machineId: String {
        switch self {
        case .task(let s): s.machineId
        case .live(let a): a.machineId
        case .ended(let e): e.machineId
        }
    }

    /// Its name within its session: a task's label, a live agent's `<space> › <agent>`, or an
    /// ended agent's Herdr name, or its kind and pane when it has none, e.g. `claude · w2:p1`.
    public var shortName: String {
        switch self {
        case .task(let s): s.label
        case .live(let a): a.displayName
        case .ended(let e): Self.short(e.name, kind: e.kind)
        }
    }

    /// The named space it runs in. A bare number, Herdr's default label, is no name, and nor is
    /// what the hub says for a Herdr app terminal outside every space.
    public var space: String? {
        let (raw, session): (String?, String?) = switch self {
        case .task(let s): (nil, s.herdrSession)
        case .live(let a): (a.space, a.herdrSession)
        case .ended(let e): (e.space, e.herdrSession)
        }
        let space = raw?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if space.isEmpty || space.allSatisfy({ $0.isASCII && $0.isNumber }) { return nil }
        return session == HerdrSession.herdrApp && space == "core" ? nil : space
    }

    /// Its name under its space's heading, which a live agent's `shortName` would repeat.
    public var nameInSpace: String {
        if case .live(let a) = self { return a.agentLabel }
        return shortName
    }

    /// What its terminal says it's working on.
    public var title: String? {
        switch self {
        case .task: nil
        case .live(let a): a.title
        case .ended(let e): e.title
        }
    }

    /// Herdr's view of it: `idle`, `working`, `blocked`, `done`, or `ended`.
    public var agentStatus: String {
        switch self {
        case .task(let s): HerdrSession.liveLifecycles.contains(s.lifecycle) ? s.agentStatus : "ended"
        case .live(let a): a.agentStatus
        case .ended: "ended"
        }
    }

    public var needsYou: Bool {
        switch self {
        case .task(let s): !s.pendingApprovals.isEmpty || s.hookFailed || s.agentStatus == "blocked"
        case .live(let a): a.agentStatus == "blocked"
        case .ended: false
        }
    }

    public var endedAt: Int64 {
        switch self {
        case .task(let s): s.updatedAt
        case .live: 0
        case .ended(let e): e.endedAt
        }
    }

    static func short(_ name: String, kind: String?) -> String {
        let last = name.split(separator: "/").last.map(String.init) ?? name
        return last.contains(":") ? "\(kind ?? "agent") · \(last)" : last
    }
}
