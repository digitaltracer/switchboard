import Foundation

/// Where home tiles get their last line: the hub's activity list for machines it covers, else
/// the agent's conversation, read at most every `FallbackPacer.interval`.
public struct TileLines: Sendable, Equatable {
    public enum Line: Equatable, Sendable {
        /// From the activity list; nil when the agent has said nothing yet.
        case known(String?)
        /// Its machine can't say, or the hub is older: read the conversation.
        case unknown
    }

    let machines: Set<String>
    let bySession: [String: String?]
    let byAgent: [String: String?]

    /// Nil when the hub sent no activity, as older hubs don't.
    public init?(_ list: Schemas.Agents) {
        guard let activity = list.activity else { return nil }
        machines = Set(list.activityMachines ?? [])
        var bySession: [String: String?] = [:]
        var byAgent: [String: String?] = [:]
        for a in activity {
            if let s = a.sessionId { bySession[s] = a.line }
            byAgent["\(a.machineId)/\(a.name)"] = a.line
        }
        self.bySession = bySession
        self.byAgent = byAgent
    }

    public func line(for agent: HerdrSession.Agent) -> Line {
        switch agent {
        case .task(let s):
            guard machines.contains(s.machineId) else { return .unknown }
            return .known(bySession[s.id] ?? nil)
        case .live(let a):
            guard machines.contains(a.machineId) else { return .unknown }
            return .known(byAgent["\(a.machineId)/\(a.name)"] ?? nil)
        case .ended:
            return .known(nil)
        }
    }
}

/// When each tile last read its conversation, so one activity doesn't cover reads it at most
/// every `interval`.
public struct FallbackPacer: Sendable {
    public static let interval: TimeInterval = 30
    var last: [String: Date] = [:]

    public init() {}

    /// Whether tile `id` may read now; if so, the read counts from `now`.
    public mutating func due(_ id: String, now: Date = Date()) -> Bool {
        if let at = last[id], now.timeIntervalSince(at) < Self.interval { return false }
        last[id] = now
        return true
    }
}
