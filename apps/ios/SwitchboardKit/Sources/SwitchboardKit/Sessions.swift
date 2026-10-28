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
        let running = all.filter { !$0.isEnded }.sorted { ($0.key.name, $0.key.machine) < ($1.key.name, $1.key.machine) }
        let done = all.filter(\.isEnded).sorted { $0.endedAt > $1.endedAt }
        return running + done
    }

    static let liveLifecycles: Set<String> = ["starting", "running", "stopping"]

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

    /// Its name within its session: the Herdr name without the session, or its kind and pane when
    /// it has none, e.g. `claude · w2:p1`.
    public var shortName: String {
        switch self {
        case .task(let s): s.label
        case .live(let a): Self.short(a.name, kind: a.kind)
        case .ended(let e): Self.short(e.name, kind: e.kind)
        }
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
