import Foundation

/// Whether the phone offers to rename something or start an agent somewhere.
public enum Offer: Equatable, Sendable {
    case yes
    /// It can't be done from the phone; the page says why.
    case no(String)
    /// Nothing to offer or say: it has ended, or the hub or the machine's runner is too old to tell.
    case hidden
}

extension HerdrSession.Agent {
    /// The id of the space it runs in on its machine, when the hub and runner say.
    public var spaceId: String? {
        switch self {
        case .task(let s): s.spaceId
        case .live(let a): a.spaceId
        case .ended: nil
        }
    }

    /// A session is renamed in the hub. An agent Switchboard didn't start is renamed where it
    /// runs, when its runner says it can be.
    public var rename: Offer {
        switch self {
        case .task(let s):
            s.canRename == true && s.lifecycle != "deleted" ? .yes : .hidden
        case .live(let a):
            switch a.canRename {
            case true?: .yes
            case false?: .no(Names.atTheMac)
            case nil: .hidden
            }
        case .ended: .hidden
        }
    }

    /// One in a Herdr terminal that Switchboard didn't start: Herdr restricts its names.
    public var isHerdrAgent: Bool {
        if case .live(let a) = self { return a.herdrSession != HerdrSession.herdrApp }
        return false
    }
}

extension AgentSpace {
    static let severalShare = "Several spaces share this name."

    /// The ids its live agents give for their space.
    private var spaceIds: Set<String?> { Set(live.map(\.spaceId)) }

    /// The one id its live agents' space goes by; nil when none says, or they disagree.
    public var spaceId: String? {
        let ids = spaceIds
        return ids.count == 1 ? ids.first ?? nil : nil
    }

    /// A Herdr app space, or a Herdr workspace the phone shows by name. A space shown under its
    /// Herdr session's name can't be renamed: Herdr has no way to rename a session.
    public var rename: Offer {
        if live.isEmpty { return .hidden }
        if !isHerdrApp, key.space == nil {
            return .no("This Herdr session's name is fixed. Rename a workspace at the Mac.")
        }
        if key.space == nil { return .hidden }
        return shared(else: "Rename one at the Mac.")
    }

    /// A new session's tab can open in a Herdr app space. In a Herdr terminal each session gets
    /// a Herdr session of its own, so nothing is added to one from the phone.
    public var newAgent: Offer {
        if live.isEmpty { return .hidden }
        if !isHerdrApp { return .no("New agents here start at the Mac.") }
        if key.space == nil { return .hidden }
        return shared(else: "Start it at the Mac.")
    }

    private func shared(else advice: String) -> Offer {
        let ids = spaceIds
        if ids == [nil] { return .hidden }
        return ids.count == 1 ? .yes : .no("\(Self.severalShare) \(advice)")
    }

    /// The project its Switchboard sessions share, to start another in; nil when they differ.
    public var project: String? {
        let projects = Set(live.compactMap { if case .task(let s) = $0 { s.projectId } else { nil } })
        return projects.count == 1 ? projects.first : nil
    }
}

/// Names the owner gives from the phone.
public enum Names {
    public static let maxLength = 40
    public static let atTheMac = "Rename it at the Mac."
    public static let herdrAgentRule = "Start with a lowercase letter, then lowercase letters, digits, - or _. 32 at most."

    /// What is wrong with a new name, or nil. `herdrAgent` is an agent in a Herdr terminal.
    public static func problem(_ name: String, herdrAgent: Bool = false) -> String? {
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        if name.isEmpty { return "Give it a name." }
        if herdrAgent {
            let rest = name.unicodeScalars.dropFirst()
            let lower = { (c: Unicode.Scalar) in c.value >= 97 && c.value <= 122 }
            let ok = name.unicodeScalars.first.map(lower) == true && name.unicodeScalars.count <= 32
                && rest.allSatisfy { lower($0) || ($0.value >= 48 && $0.value <= 57) || $0 == "-" || $0 == "_" }
            return ok ? nil : herdrAgentRule
        }
        if name.count > maxLength { return "Keep it to \(maxLength) characters." }
        if name.unicodeScalars.contains(where: { $0 == "/" || $0.properties.generalCategory == .control }) {
            return "A name can't contain a slash or a line break."
        }
        return nil
    }

    /// A rename that failed, in words for the sheet.
    public static func failure(_ error: Error) -> String {
        guard let e = error as? HubError else { return error.localizedDescription }
        switch e.code {
        case "name_taken": return "That name is taken."
        case "machine_offline": return "That machine is offline."
        case "not_found", "unknown_session": return "It's no longer there."
        case "busy": return "The layout changed. Try again."
        case "runner_too_old": return "Update Switchboard on that machine first."
        case "outcome_unknown": return "No answer. Check the name in a moment."
        default: return e.localizedDescription
        }
    }
}
