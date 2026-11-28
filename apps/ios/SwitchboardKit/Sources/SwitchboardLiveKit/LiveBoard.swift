import Foundation

/// What the lock-screen card shows, as `src/live.rs` builds it. Short keys keep it inside a push.
public struct LiveBoard: Codable, Equatable, Hashable, Sendable {
    public struct Waiting: Codable, Equatable, Hashable, Sendable {
        /// The agent's Herdr name; never its model.
        public var name: String
        public var space: String?
        public var since: Date
        public var error: Bool

        public init(name: String, space: String? = nil, since: Date, error: Bool = false) {
            self.name = name
            self.space = space
            self.since = since
            self.error = error
        }

        enum CodingKeys: String, CodingKey { case name = "a", space = "s", since = "w", error = "e" }

        public init(from d: Decoder) throws {
            let c = try d.container(keyedBy: CodingKeys.self)
            name = try c.decode(String.self, forKey: .name)
            space = try c.decodeIfPresent(String.self, forKey: .space)
            since = Date(timeIntervalSince1970: try c.decode(Double.self, forKey: .since))
            error = try c.decodeIfPresent(Bool.self, forKey: .error) ?? false
        }

        public func encode(to e: Encoder) throws {
            var c = e.container(keyedBy: CodingKeys.self)
            try c.encode(name, forKey: .name)
            try c.encodeIfPresent(space, forKey: .space)
            try c.encode(Int64(since.timeIntervalSince1970), forKey: .since)
            if error { try c.encode(true, forKey: .error) }
        }
    }

    public var at: Date
    /// Longest wait first.
    public var waiting: [Waiting]
    /// How many more wait beyond `waiting`.
    public var more: Int
    public var running: [String]
    public var runningMore: Int
    public var idle: Int
    public var finished: String?
    public var finishedAt: Date?

    public init(at: Date = Date(), waiting: [Waiting] = [], more: Int = 0, running: [String] = [], runningMore: Int = 0,
                idle: Int = 0, finished: String? = nil, finishedAt: Date? = nil) {
        self.at = at
        self.waiting = waiting
        self.more = more
        self.running = running
        self.runningMore = runningMore
        self.idle = idle
        self.finished = finished
        self.finishedAt = finishedAt
    }

    enum CodingKeys: String, CodingKey {
        case version = "v", at = "t", waiting = "n", more = "m", running = "r", runningMore = "rm", idle = "i"
        case finished = "f", finishedAt = "fa"
    }

    public init(from d: Decoder) throws {
        let c = try d.container(keyedBy: CodingKeys.self)
        at = Date(timeIntervalSince1970: try c.decode(Double.self, forKey: .at))
        waiting = try c.decodeIfPresent([Waiting].self, forKey: .waiting) ?? []
        more = try c.decodeIfPresent(Int.self, forKey: .more) ?? 0
        running = try c.decodeIfPresent([String].self, forKey: .running) ?? []
        runningMore = try c.decodeIfPresent(Int.self, forKey: .runningMore) ?? 0
        idle = try c.decodeIfPresent(Int.self, forKey: .idle) ?? 0
        finished = try c.decodeIfPresent(String.self, forKey: .finished)
        finishedAt = try c.decodeIfPresent(Double.self, forKey: .finishedAt).map(Date.init(timeIntervalSince1970:))
    }

    public func encode(to e: Encoder) throws {
        var c = e.container(keyedBy: CodingKeys.self)
        try c.encode(1, forKey: .version)
        try c.encode(Int64(at.timeIntervalSince1970), forKey: .at)
        try c.encode(waiting, forKey: .waiting)
        if more > 0 { try c.encode(more, forKey: .more) }
        try c.encode(running, forKey: .running)
        if runningMore > 0 { try c.encode(runningMore, forKey: .runningMore) }
        try c.encode(idle, forKey: .idle)
        try c.encodeIfPresent(finished, forKey: .finished)
        try c.encodeIfPresent(finishedAt.map { Int64($0.timeIntervalSince1970) }, forKey: .finishedAt)
    }

    public var runningCount: Int { running.count + runningMore }
    public var waitingCount: Int { waiting.count + more }
}

/// When the lock-screen card shows, chosen per phone and kept on the hub with its registration.
public enum LiveShowMode: String, CaseIterable, Sendable {
    /// While something works or waits; it ends fifteen minutes after everything is idle.
    case working
    /// Always, with "All quiet" when nothing runs.
    case always

    public static let key = "liveShowMode"
    /// A card ends after this long with nothing working, waiting or just finished.
    public static let quietEnd: TimeInterval = 15 * 60

    public static var current: LiveShowMode {
        UserDefaults.standard.string(forKey: key).flatMap(LiveShowMode.init) ?? .working
    }

    /// What to do with the card for this board: `quietSince` is when the board went quiet.
    public func decide(_ board: LiveBoard, cardShowing: Bool, quietSince: Date?, now: Date = Date()) -> LiveCardStep {
        if self == .always { return .show }
        if !board.isQuiet { return .show }
        guard cardShowing else { return .none }
        if let q = quietSince, now.timeIntervalSince(q) >= Self.quietEnd { return .end }
        return .show
    }
}

public enum LiveCardStep: Equatable, Sendable { case show, end, none }

/// The one card to keep when iOS shows several; the others are ended.
public enum LiveCards {
    /// The newest by start, a card first seen now counting as new; on a tie, the later in `ids`.
    public static func keep(_ ids: [String], started: [String: Date], now: Date = Date()) -> String? {
        var best: (id: String, at: Date)?
        for id in ids {
            let at = started[id] ?? now
            if best.map({ at >= $0.at }) ?? true { best = (id, at) }
        }
        return best?.id
    }
}

/// Which of the card's states a board is in, as the design names them.
public enum LiveMood: Equatable, Sendable {
    case calm
    case needsYou(LiveBoard.Waiting, others: Int)
    case error(LiveBoard.Waiting, others: Int)
    case finished(String)

    /// A finished agent shows for about a minute, then the card returns to calm.
    public static let finishedFor: TimeInterval = 60

    public init(_ b: LiveBoard, now: Date = Date()) {
        if let first = b.waiting.first {
            let others = b.waitingCount - 1
            self = first.error ? .error(first, others: others) : .needsYou(first, others: others)
        } else if let f = b.finished, let at = b.finishedAt, now.timeIntervalSince(at) < Self.finishedFor {
            self = .finished(f)
        } else {
            self = .calm
        }
    }

    /// Pushed at high priority, with a generic alert.
    public var urgent: Bool {
        switch self {
        case .needsYou, .error: true
        default: false
        }
    }
}

extension LiveBoard {
    /// No agents at all.
    public var isEmpty: Bool { waiting.isEmpty && runningCount == 0 && idle == 0 }

    /// Nothing working, waiting or just finished, as the hub's `Board::is_quiet` says.
    public var isQuiet: Bool { waitingCount == 0 && runningCount == 0 && finished == nil }

    /// Whether two boards show the same thing, whenever each was built.
    public func sameAs(_ other: LiveBoard) -> Bool {
        var a = self, b = other
        a.at = .distantPast
        b.at = .distantPast
        return a == b
    }
}
