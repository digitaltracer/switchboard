import Foundation

/// One group of agents as the phone lists it, wherever it lives: a space in the Herdr app, or a
/// workspace of a Herdr terminal session, `session › workspace`. A terminal session with one
/// workspace, or only numbered ones, is one space under the session's name; its numbered
/// workspaces otherwise go under the session's name alone.
public struct AgentSpace: Identifiable, Hashable, Sendable {
    public struct Key: Hashable, Sendable {
        public let machine: String
        /// The Herdr session, `hc` for the Herdr app.
        public let session: String
        /// The space's name; nil for the whole of a terminal session, or the Herdr app's agents
        /// outside every space.
        public let space: String?

        public init(machine: String, session: String, space: String?) {
            self.machine = machine
            self.session = session
            self.space = space
        }
    }

    public let key: Key
    /// `atlas-web`, `main › kraft`, `kraft-7c1e`.
    public let title: String
    public internal(set) var live: [HerdrSession.Agent] = []
    public internal(set) var ended: [HerdrSession.Agent] = []

    public var id: Key { key }
    public var isHerdrApp: Bool { key.session == HerdrSession.herdrApp }
    /// `hc` for the Herdr app, `herdr` for a Herdr terminal.
    public var backend: String { isHerdrApp ? "hc" : "herdr" }
    public var isEnded: Bool { live.isEmpty }
    public var agents: [HerdrSession.Agent] { live + ended }
    public var endedAt: Int64 { ended.map(\.endedAt).max() ?? 0 }
    /// Holds a session Switchboard started.
    public var fromSwitchboard: Bool {
        agents.contains { if case .task = $0 { true } else { false } }
    }
    /// The Herdr terminal session to attach to; none for the Herdr app.
    public var attach: String? { isHerdrApp ? nil : key.session }

    /// The name to show for one of its agents elsewhere, e.g. `atlas-web › search`; just the
    /// space's title when they're the same, as for a session Switchboard started on its own.
    public func label(_ a: HerdrSession.Agent) -> String {
        let name = a.nameInSpace
        return name == title ? title : "\(title) › \(name)"
    }

    /// Every space, live ones in title order, then those that ended, latest first.
    public static func group(sessions: [Schemas.Session], agents: [Schemas.AgentView],
                             ended: [Schemas.EndedAgent], machine: String? = nil,
                             since: Int64?) -> [AgentSpace] {
        let all = HerdrSession.group(sessions: sessions, agents: agents, ended: ended, machine: machine, since: since)
            .flatMap(split)
        let live = all.filter { !$0.isEnded }
            .sorted { ($0.title.lowercased(), $0.key.machine) < ($1.title.lowercased(), $1.key.machine) }
        let done = all.filter(\.isEnded).sorted { $0.endedAt > $1.endedAt }
        return live + done
    }

    static func split(_ g: HerdrSession) -> [AgentSpace] {
        let m = g.key.machine
        let spaces = g.spaces
        if g.isHerdrApp {
            return spaces.map { s in
                AgentSpace(key: Key(machine: m, session: g.key.name, space: s.name),
                           title: s.name ?? "Outside any space", live: s.live, ended: s.ended)
            }
        }
        let named = spaces.filter { $0.name != nil }
        let unnamed = spaces.contains { $0.name == nil }
        if named.isEmpty || (named.count == 1 && !unnamed) {
            return [AgentSpace(key: Key(machine: m, session: g.key.name, space: nil), title: g.title, live: g.live, ended: g.ended)]
        }
        return spaces.map { s in
            AgentSpace(key: Key(machine: m, session: g.key.name, space: s.name),
                       title: s.name.map { "\(g.title) › \($0)" } ?? g.title, live: s.live, ended: s.ended)
        }
    }
}

extension Array where Element == AgentSpace {
    /// The space holding the agent `match` picks out.
    public func first(holding match: (HerdrSession.Agent) -> Bool) -> AgentSpace? {
        first { $0.agents.contains(where: match) }
    }
}
