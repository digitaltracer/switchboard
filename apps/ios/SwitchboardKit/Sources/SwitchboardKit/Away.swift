import Foundation

/// "While you were away": since this phone last had the app open.
public enum AwayDigest {
    /// Shorter gaps aren't "away".
    public static let minAway: TimeInterval = 10 * 60

    /// "1 h 40 m", "12 min", "under a minute".
    public static func duration(_ seconds: TimeInterval) -> String {
        let m = Int(seconds / 60)
        if m < 1 { return "under a minute" }
        if m < 60 { return "\(m) min" }
        return m % 60 == 0 ? "\(m / 60) h" : "\(m / 60) h \(m % 60) m"
    }

    /// One line for the Inbox card: what it's doing now, or what it last did.
    public static func line(_ a: Schemas.AwayAgentView, now: Int64) -> String {
        let reply = a.lastReply.map { "\"\($0)\"" }
        let last = a.facts.last
        let since = { (kind: String) -> String in
            let at = a.facts.last { $0.kind == kind }?.at ?? now
            return duration(TimeInterval(now - at) / 1000)
        }
        switch a.state {
        case "waiting":
            return "Waiting \(since("waiting"))" + (reply.map { ": \($0)" } ?? "")
        case "error":
            return "Stopped with an error"
        case "working":
            let allowed = a.facts.filter { $0.kind == "allowed" }.count
            return "Still working" + (allowed > 0 ? " · you allowed \(allowed) \(allowed == 1 ? "command" : "commands") meanwhile" : "")
        default:
            if a.facts.contains(where: { $0.kind == "finished" }) {
                return "Finished" + (reply.map { ": \($0)" } ?? "")
            }
            return last?.text ?? "Nothing new"
        }
    }

    /// Its dot: needs you, stopped, done or working.
    public static func tone(_ a: Schemas.AwayAgentView) -> String {
        switch a.state {
        case "waiting": "need"
        case "error": "bad"
        case "working": "working"
        default: "done"
        }
    }
}

extension AwayDigest {
    /// Rows the agent page shows at most.
    public static let maxRows = 5

    /// The facts as a short story: consecutive turns collapse into one "Finished" row at the last
    /// finish, the "Started working" between them goes, and only the newest rows are kept.
    public static func timeline(_ facts: [Schemas.AwayFactView]) -> [Schemas.AwayFactView] {
        var out: [Schemas.AwayFactView] = []
        var turns = 0
        var pending: Schemas.AwayFactView?
        func flush() {
            guard let p = pending else { return }
            out.append(.init(at: p.at, kind: "finished", text: turns == 1 ? "Finished its turn" : "Finished \(turns) turns"))
            pending = nil
            turns = 0
        }
        for f in facts {
            switch f.kind {
            case "working" where pending != nil || out.last?.kind == "working":
                continue
            case "finished":
                if out.last?.kind == "working", pending == nil { out.removeLast() }
                turns += 1
                pending = f
            default:
                flush()
                out.append(f)
            }
        }
        flush()
        return Array(out.suffix(maxRows))
    }
}
