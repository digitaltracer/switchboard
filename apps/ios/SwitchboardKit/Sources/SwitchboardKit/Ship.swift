import Foundation

/// A pull request named in an agent's reply. Switchboard never calls GitHub: this is all it knows.
public struct PullRequest: Equatable, Sendable {
    public var url: URL
    public var number: Int
    /// `owner/repo`.
    public var repo: String
    /// The link's text when the reply wrote it as `[title](url)`.
    public var title: String?

    private static let pattern = try! NSRegularExpression(
        pattern: #"https://(?:github\.com/([\w.-]+/[\w.-]+)/pull/(\d+)|gitlab\.com/([\w./-]+?)/-/merge_requests/(\d+)|bitbucket\.org/([\w.-]+/[\w.-]+)/pull-requests/(\d+))"#
    )

    /// The last pull request link in `text`.
    public static func find(in text: String) -> PullRequest? {
        let ns = text as NSString
        guard let m = pattern.matches(in: text, range: NSRange(location: 0, length: ns.length)).last else { return nil }
        let group = stride(from: 1, to: 7, by: 2).first { m.range(at: $0).location != NSNotFound } ?? 1
        guard let url = URL(string: ns.substring(with: m.range)),
              let number = Int(ns.substring(with: m.range(at: group + 1))) else { return nil }
        let before = ns.substring(to: m.range.location)
        var title: String?
        if before.hasSuffix("]("), let open = before.dropLast(2).lastIndex(of: "[") {
            let t = before[before.index(after: open)..<before.index(before.endIndex, offsetBy: -2)]
            if !t.contains("\n"), !t.isEmpty, !t.hasPrefix("http") { title = String(t) }
        }
        return PullRequest(url: url, number: number, repo: ns.substring(with: m.range(at: group)), title: title)
    }
}

public struct ShipButton: Equatable, Hashable, Sendable, Identifiable {
    /// As the agent runs it: `/pr` for Claude Code, `$pr` for a Codex skill.
    public var skill: String
    public var label: String
    public var afterPR: Bool
    public var id: String { skill }

    public init(skill: String, label: String, afterPR: Bool = false) {
        self.skill = skill
        self.label = label
        self.afterPR = afterPR
    }

    public init(_ v: Schemas.ShipButtonView) {
        self.init(skill: v.skill, label: v.label, afterPR: v.afterPr ?? false)
    }

    public var view: Schemas.ShipButtonView { .init(skill: skill, label: label, afterPr: afterPR) }

    /// What the agent gets: the skill, then any instructions.
    public func message(_ instructions: String = "") -> String {
        let extra = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
        return extra.isEmpty ? skill : "\(skill) \(extra)"
    }

    /// The name without its sigil.
    public var base: String { ShipButtons.base(skill) }

    /// The same skill as this agent lists it, so a `/pr` button runs `$pr` on a Codex agent.
    public func invocation(for skills: [String]) -> ShipButton {
        guard !skills.contains(skill), let theirs = skills.first(where: { ShipButtons.base($0) == base }) else { return self }
        var b = self
        b.skill = theirs
        return b
    }

    /// Whether running it should end with a pull request, so its absence is worth saying.
    public var opensPR: Bool {
        let words = base.lowercased().split(whereSeparator: { !$0.isLetter })
        return words.contains("pr") || words.contains("ship") || base.lowercased().contains("pull")
    }
}

public enum ShipButtons {
    /// The project's own list, else the one for all projects.
    public static func effective(_ v: Schemas.ShipButtons, project: String?) -> [ShipButton] {
        let own = project.flatMap { p in v.projects.first { $0.project == p } }
        return (own?.buttons ?? v.all).map(ShipButton.init)
    }

    /// Before the owner picks any: the usual ship skills this agent actually has, as it names them.
    public static func suggested(from skills: [String]) -> [ShipButton] {
        defaults.compactMap { d in skills.first { base($0) == d.skill }.map { ShipButton(skill: $0, label: d.label, afterPR: d.afterPR) } }
    }

    public static func base(_ skill: String) -> String {
        skill.first.map { "/$".contains($0) } == true ? String(skill.dropFirst()) : skill
    }

    /// By base name.
    static let defaults: [ShipButton] = [
        .init(skill: "pr", label: "Raise PR"),
        .init(skill: "ship", label: "Ship"),
        .init(skill: "commit", label: "Commit only"),
        .init(skill: "code-review", label: "Review first"),
        .init(skill: "triage-reviews", label: "Triage reviews", afterPR: true),
        .init(skill: "fix-ci", label: "Fix CI", afterPR: true),
    ]

    /// A label for a skill the owner adds: the known one, else its name in words.
    public static func label(for skill: String) -> String {
        let name = base(skill)
        if let d = defaults.first(where: { $0.skill == name }) { return d.label }
        let base = name.split(separator: ":").last.map(String.init) ?? name
        let words = base.replacingOccurrences(of: "-", with: " ").replacingOccurrences(of: "_", with: " ")
        return words.prefix(1).uppercased() + words.dropFirst()
    }

    public static func afterPRByDefault(_ skill: String) -> Bool {
        defaults.first { $0.skill == base(skill) }?.afterPR ?? false
    }
}

/// What a ship skill came to, from the agent's latest reply once its turn ended.
public enum ShipOutcome: Equatable, Sendable {
    case running
    case opened(PullRequest)
    /// It was meant to open one and didn't; the reply says why.
    case noPullRequest
    case done

    public static func of(sent: ShipButton, turnEnded: Bool, reply: String?) -> ShipOutcome {
        if let pr = reply.flatMap(PullRequest.find(in:)) { return .opened(pr) }
        guard turnEnded else { return .running }
        return sent.opensPR ? .noPullRequest : .done
    }
}
