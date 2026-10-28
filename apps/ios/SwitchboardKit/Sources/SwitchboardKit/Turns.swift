import Foundation

/// A conversation as a phone shows it: your messages and the agent's replies, with the steps in
/// between folded into one piece of work that says what came of it.
public enum Turns {
    public enum Entry: Identifiable, Equatable, Sendable {
        case user(Conversation.Item)
        case reply(Conversation.Item)
        /// Something the agent said with more work after it before your next message: a note on the way.
        case progress(Conversation.Item)
        case work(Work)
        case note(Conversation.Item)

        public var id: String {
            switch self {
            case .user(let i), .reply(let i), .progress(let i), .note(let i): i.id
            case .work(let w): "work:\(w.items.first?.id ?? "")"
            }
        }
    }

    public struct Work: Equatable, Sendable {
        public let items: [Conversation.Item]
        /// From its first step to its last, when the log has times.
        public let seconds: Int?
        public let tags: [Tag]
        public var running: Bool { items.contains { !$0.done } }
    }

    public struct Tag: Equatable, Sendable {
        public enum Tone: Sendable { case plain, good, bad }
        public let text: String
        public var added: Int? = nil
        public var removed: Int? = nil
        public var tone: Tone = .plain
    }

    public static func entries(_ items: [Conversation.Item]) -> [Entry] {
        var out: [Entry] = []
        var steps: [Conversation.Item] = []
        func flush() {
            if !steps.isEmpty { out.append(.work(work(steps))) }
            steps = []
        }
        for item in items {
            switch item.kind {
            case "tool": steps.append(item)
            case "user": flush(); out.append(.user(item))
            case "assistant": flush(); out.append(.reply(item))
            default: flush(); out.append(.note(item))
            }
        }
        flush()
        // A reply with work or another reply after it, before your next message, was on the way.
        var followed = false
        for i in out.indices.reversed() {
            switch out[i] {
            case .user: followed = false
            case .work: followed = true
            case .reply(let item):
                if followed { out[i] = .progress(item) } else { followed = true }
            default: break
            }
        }
        return out
    }

    /// Since your last message: the agent's latest reply and what its work came to.
    public static func latest(_ items: [Conversation.Item]) -> (reply: Conversation.Item?, tags: [Tag], at: String?) {
        let since = items.lastIndex { $0.kind == "user" }.map { items.index(after: $0) } ?? items.startIndex
        let recent = Array(items[since...])
        let reply = recent.last { $0.kind == "assistant" } ?? items.last { $0.kind == "assistant" }
        let steps = recent.filter { $0.kind == "tool" }
        return (reply, steps.isEmpty ? [] : tags(steps), reply?.at)
    }

    static func work(_ steps: [Conversation.Item]) -> Work {
        let times = steps.compactMap { $0.at.flatMap(Self.date) }
        let seconds = times.min().flatMap { first in times.max().map { Int($0.timeIntervalSince(first)) } }
        return Work(items: steps, seconds: seconds, tags: tags(steps))
    }

    public static func tags(_ steps: [Conversation.Item]) -> [Tag] {
        var out: [Tag] = []
        let edits = steps.filter(isEdit)
        if !edits.isEmpty {
            let files = Set(edits.map { $0.text ?? $0.id }).count
            let added = edits.compactMap(\.added).reduce(0, +)
            let removed = edits.compactMap(\.removed).reduce(0, +)
            let counted = edits.contains { $0.added != nil || $0.removed != nil }
            out.append(Tag(text: files == 1 ? "1 file" : "\(files) files",
                           added: counted ? Int(added) : nil, removed: counted ? Int(removed) : nil))
        }
        let tests = steps.filter { isRun($0) && test.firstMatch(in: $0.text ?? "") != nil }
        if !tests.isEmpty, tests.allSatisfy(\.done) {
            out.append(tests.contains(where: \.failed) ? Tag(text: "Tests failed", tone: .bad) : Tag(text: "Tests passed", tone: .good))
        }
        if let pr = steps.lazy.compactMap({ pullRequest($0) }).last { out.append(Tag(text: "PR #\(pr)")) }
        if steps.contains(where: { isRun($0) && ($0.text ?? "").contains("git push") && $0.done && !$0.failed }) {
            out.append(Tag(text: "Pushed"))
        }
        let failed = steps.filter { $0.failed && !tests.contains($0) }.count
        if failed > 0 { out.append(Tag(text: "\(failed) failed", tone: .bad)) }
        if out.isEmpty {
            let runs = steps.filter(isRun).count
            out.append(Tag(text: runs > 0 ? (runs == 1 ? "1 command" : "\(runs) commands") : (steps.count == 1 ? "1 step" : "\(steps.count) steps")))
        }
        return out
    }

    static func isEdit(_ i: Conversation.Item) -> Bool {
        ["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"].contains(i.tool ?? "")
    }

    static func isRun(_ i: Conversation.Item) -> Bool { (i.text ?? "").hasPrefix("Ran ") }

    static let test = Pattern(#"\b(pytest|cargo test|cargo nextest|go test|swift test|npm (run )?test|yarn test|pnpm test|jest|vitest|rspec|mvn test|gradlew\b.*\btest)"#)
    static let pull = Pattern(#"github\.com/[^/\s]+/[^/\s]+/pull/(\d+)"#)

    static func pullRequest(_ i: Conversation.Item) -> String? {
        for text in [i.output, i.text].compactMap({ $0 }) {
            if let m = pull.firstMatch(in: text) { return m }
        }
        return nil
    }

    static func date(_ s: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f.date(from: s) ?? ISO8601DateFormatter().date(from: s)
    }

    /// When something was written, as the phone's clock shows times: "7:55 PM".
    public static func time(_ at: String) -> String? {
        date(at).map { $0.formatted(date: .omitted, time: .shortened) }
    }

    /// "6m", "9s" or "1h 5m".
    public static func duration(_ seconds: Int) -> String {
        switch seconds {
        case ..<60: "\(max(seconds, 1))s"
        case ..<3600: "\(seconds / 60)m"
        default: "\(seconds / 3600)h \(seconds % 3600 / 60)m"
        }
    }

    struct Pattern: Sendable {
        let regex: NSRegularExpression
        init(_ p: String) { regex = try! NSRegularExpression(pattern: p) }
        /// The first capture group, or the whole match.
        func firstMatch(in s: String) -> String? {
            guard let m = regex.firstMatch(in: s, range: NSRange(s.startIndex..., in: s)) else { return nil }
            let r = m.numberOfRanges > 1 && m.range(at: 1).location != NSNotFound ? m.range(at: 1) : m.range
            return Range(r, in: s).map { String(s[$0]) }
        }
    }
}

/// The skills an agent can run, filtered as you type after `/`.
public enum SkillSearch {
    /// Names starting with what's typed first, then names containing it, then descriptions.
    public static func filter(_ skills: [Schemas.Skill], typed: String) -> [Schemas.Skill] {
        let q = typed.drop { $0 == "/" || $0 == "$" }.lowercased()
        guard !q.isEmpty else { return skills }
        let bare = { (s: Schemas.Skill) in s.name.drop { $0 == "/" || $0 == "$" }.lowercased() }
        let starts = skills.filter { bare($0).hasPrefix(q) }
        let inName = skills.filter { !bare($0).hasPrefix(q) && bare($0).contains(q) }
        let inDescription = skills.filter { !bare($0).contains(q) && $0.description.lowercased().contains(q) }
        return starts + inName + inDescription
    }

    /// Whether a draft is a skill being typed: a `/` or `$` and no space yet.
    public static func isTyping(_ text: String) -> Bool {
        guard let first = text.first, first == "/" || first == "$" else { return false }
        return !text.contains(" ") && !text.contains("\n")
    }
}
