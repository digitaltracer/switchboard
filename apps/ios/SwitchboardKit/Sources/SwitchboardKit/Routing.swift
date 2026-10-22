import Foundation

/// One place a message can go, as the review sheet lists it under "Goes to".
public struct RouteOption: Identifiable, Equatable, Sendable {
    public enum Action: Equatable, Sendable {
        /// Send as is: the hub routes it the way its preview said.
        case asPreviewed
        case start(projectId: String)
        case send(Schemas.TargetView)
    }

    public let id: String
    public let title: String
    public let detail: String
    /// Starts a new session rather than continuing one.
    public let isNew: Bool
    public let action: Action
    /// What the send button says for this option.
    public let verb: String
}

/// The hub's own choice first, then the plausible alternatives: a new session in the project of
/// what it would continue, or the running sessions of the project it would start in.
public enum RouteOptions {
    public struct Result: Equatable, Sendable {
        public let options: [RouteOption]
        /// The option picked until the user picks another; none when the hub has to ask.
        public let suggested: String?

        public init(options: [RouteOption], suggested: String?) {
            self.options = options
            self.suggested = suggested
        }
    }

    static let maxAlternatives = 3

    public static func build(
        preview: Schemas.MessagePreview, sessions: [Schemas.Session], context: Schemas.TargetView?
    ) -> Result {
        let live = sessions.filter { ["running", "starting"].contains($0.lifecycle) }
        func project(_ id: String) -> String? { sessions.first { $0.id == id }?.projectId }
        var top: RouteOption?
        var rest: [RouteOption] = []
        switch preview.outcome {
        case "start":
            let p = preview.projectId ?? ""
            top = startOption(p, machine: preview.machineId, action: .asPreviewed)
            rest = live.filter { $0.projectId == p }.map { sendOption(.session($0), status: $0.agentStatus) }
        case "send", "select":
            if let t = preview.target {
                top = sendOption(t, status: t.sessionId.flatMap { id in live.first { $0.id == id }?.agentStatus }, action: .asPreviewed)
                if let p = t.sessionId.flatMap(project) { rest = [startOption(p, machine: t.machineId, action: .start(projectId: p))] }
            }
        case "choose_project":
            rest = preview.options.compactMap { o in o.projectId.map { startOption($0, machine: nil, action: .start(projectId: $0)) } }
            rest += live.prefix(maxAlternatives).map { sendOption(.session($0), status: $0.agentStatus) }
        case "choose_target":
            rest = preview.options.compactMap { o in o.target.map { sendOption($0, status: nil) } }
        default:
            break
        }
        if let context, !matches(top, context) {
            rest.insert(sendOption(context, status: context.sessionId.flatMap { id in live.first { $0.id == id }?.agentStatus }), at: 0)
        }
        var seen = Set(top.map { [$0.id] } ?? [])
        rest = rest.filter { seen.insert($0.id).inserted }
        if preview.outcome == "start" || preview.outcome == "send" || preview.outcome == "select" {
            rest = Array(rest.prefix(maxAlternatives))
        }
        return Result(options: (top.map { [$0] } ?? []) + rest, suggested: top?.id)
    }

    private static func matches(_ o: RouteOption?, _ t: Schemas.TargetView) -> Bool {
        o?.id == sendId(t)
    }

    private static func sendId(_ t: Schemas.TargetView) -> String {
        "send:" + (t.sessionId ?? "\(t.machineId)/\(t.agent ?? t.label)")
    }

    private static func startOption(_ project: String, machine: String?, action: RouteOption.Action) -> RouteOption {
        RouteOption(
            id: "start:\(project)", title: "New task in \(project)",
            detail: [machine.map { "On \($0)" }, "its own branch"].compactMap { $0 }.joined(separator: " · "),
            isNew: true, action: action, verb: "Start a task in \(project)"
        )
    }

    private static func sendOption(_ t: Schemas.TargetView, status: String?, action: RouteOption.Action? = nil) -> RouteOption {
        let what = t.kind == "agent" ? "Your Herdr session" : (status.map { $0.prefix(1).uppercased() + $0.dropFirst() } ?? "Running")
        return RouteOption(
            id: sendId(t), title: "Continue \(t.label)", detail: "\(what) · \(t.machineId)",
            isNew: false, action: action ?? .send(t), verb: "Send to \(t.label)"
        )
    }
}
