import SwiftUI
import SwitchboardKit

extension AgentSpace {
    /// "3 agents · 1 needs you · 1 working", or when it ended.
    var summary: String {
        if isEnded { return "Ended \(Ago.long(endedAt))" }
        var parts = ["\(live.count) \(live.count == 1 ? "agent" : "agents")"]
        let needs = live.filter(\.needsYou).count
        let working = live.filter { $0.agentStatus == "working" }.count
        let done = live.filter { $0.agentStatus == "done" }.count
        if live.count == 1 {
            parts.append(needs > 0 ? "needs you" : working > 0 ? "working" : done > 0 ? "done" : "idle")
        } else {
            if needs > 0 { parts.append("\(needs) need\(needs == 1 ? "s" : "") you") }
            if working > 0 { parts.append("\(working) working") }
            if needs == 0 && working == 0 { parts.append(done > 0 ? "\(done) done" : "all idle") }
        }
        if fromSwitchboard { parts.append("from Switchboard") }
        return parts.joined(separator: " · ")
    }

    var glyph: Glyph {
        if isEnded { return .end }
        if live.contains(where: \.needsYou) { return .need }
        if live.contains(where: { $0.agentStatus == "working" }) { return .work }
        if live.contains(where: { $0.agentStatus == "done" }) { return .done }
        return .idle
    }
}

extension HerdrSession.Agent {
    var status: SessionStatus {
        switch self {
        case .task(let s): SessionStatus(s)
        case .live(let a): SessionStatus(agentStatus: a.agentStatus)
        case .ended: .ended
        }
    }

    var isEnded: Bool { agentStatus == "ended" }

    var backend: String {
        switch self {
        case .task(let s): s.backend ?? "herdr"
        case .live(let a): a.backend ?? "herdr"
        case .ended(let e): e.herdrSession == HerdrSession.herdrApp ? "hc" : "herdr"
        }
    }

    /// "Working · Add typeahead", "Needs you · Claude · sb/7c1e", or "Ended 2h ago · …".
    var subtitle: String {
        switch self {
        case .task(let s):
            let when = isEnded ? "Ended \(Ago.long(s.updatedAt))" : status.text
            return [when, s.agent == "codex" ? "Codex" : "Claude Code", s.branch].compactMap { $0 }.joined(separator: " · ")
        case .live(let a):
            let detail = a.isNamed ? a.shownTitle ?? a.kind?.capitalized : a.shownTitle.flatMap { _ in a.kind?.capitalized }
            return [status.text, detail].compactMap { $0 }.joined(separator: " · ")
        case .ended(let e):
            return ["Ended \(Ago.long(e.endedAt))", e.title ?? e.kind?.capitalized].compactMap { $0 }.joined(separator: " · ")
        }
    }
}

/// A link to the agent's page, or just the label for one that ended outside Switchboard. Each
/// value keeps its own type: one typed `AnyHashable` matches no destination, so the tap does nothing.
struct AgentLink<Label: View>: View {
    let agent: HerdrSession.Agent
    @ViewBuilder let label: Label

    var body: some View {
        Group {
            switch agent {
            case .task(let s): NavigationLink(value: s.id) { label }
            case .live(let a): NavigationLink(value: AgentRef(machine: a.machineId, name: a.name)) { label }
            case .ended: label
            }
        }
        .renameMenu(agent: agent)
    }
}

/// One space, as a row of the Spaces list.
struct SpaceRow: View {
    let space: AgentSpace
    var showMachine = false
    var first = false

    var body: some View {
        NavigationLink(value: space.key) {
            Row(title: space.title, subtitle: showMachine ? "\(space.summary) · \(space.key.machine)" : space.summary,
                dim: space.isEnded, first: first) {
                StatusGlyph(glyph: space.glyph)
            } trailing: {
                WhereTag(backend: space.backend)
            }
        }
        .buttonStyle(.plain)
        .renameMenu(space: space)
        .accessibilityIdentifier("space-\(space.title)")
    }
}

/// Every live space with its counts, a filter by where they live, and the ended ones below.
struct SpacesList: View {
    @Environment(AppModel.self) private var model
    var machine: String?
    @State private var filter = "all"

    var body: some View {
        let all = model.spaces(machine: machine, since: nil)
        let live = all.filter { !$0.isEnded }
        let shown = live.filter { filter == "all" || $0.backend == filter }
        let ended = all.filter { $0.isEnded && (filter == "all" || $0.backend == filter) }
        let machines = Set(all.map(\.key.machine)).count > 1 && machine == nil
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                chip("all", "All \(live.count)")
                chip("hc", "App \(live.filter(\.isHerdrApp).count)")
                chip("herdr", "Terminal \(live.filter { !$0.isHerdrApp }.count)")
                Spacer(minLength: 0)
                if machine == nil {
                    NavigationLink(value: Browse.machines) {
                        Text("Machines").font(Face.geist(13.5, .medium)).foregroundStyle(Ink.ac).frame(minHeight: 44)
                    }
                    .accessibilityIdentifier("machinesLink")
                }
            }
            .padding(.bottom, 8)
            if shown.isEmpty {
                StateRow(text: live.isEmpty ? "No spaces are live." : "None live here.")
            }
            ForEach(Array(shown.enumerated()), id: \.element.key) { i, s in
                SpaceRow(space: s, showMachine: machines, first: i == 0)
            }
            if !ended.isEmpty {
                SectionHead("Ended", count: ended.count)
                ForEach(Array(ended.prefix(30).enumerated()), id: \.element.key) { i, s in
                    SpaceRow(space: s, showMachine: machines, first: i == 0)
                }
            }
        }
    }

    private func chip(_ id: String, _ label: String) -> some View {
        Button { filter = id } label: { Chip(text: label, on: filter == id) }
            .buttonStyle(.plain)
            .frame(minHeight: 44)
            .accessibilityIdentifier("filter-\(id)")
    }
}

/// One space: its agents, those that ended today, and where to find it at your desk.
struct SpacePage: View {
    @Environment(AppModel.self) private var model
    @Environment(\.replaceTop) private var replaceTop
    let key: AgentSpace.Key
    @State private var renaming = false
    @State private var starting = false
    /// One request id per name tried, so a retry after a lost answer is the same request.
    @State private var renameIds: [String: String] = [:]

    private var space: AgentSpace? {
        model.spaces(machine: key.machine, since: HerdrSession.startOfToday()).first { $0.key == key }
            ?? model.spaces(machine: key.machine, since: nil).first { $0.key == key }
    }

    var body: some View {
        let space = space
        VStack(spacing: 0) {
            PageTop(back: "Back to spaces", meta: key.machine)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    let n = space?.live.count ?? 0
                    Headline("\(space?.title ?? key.space ?? key.session).", n == 0 ? "Nothing running." : "\(n) \(n == 1 ? "agent" : "agents").")
                        .stateStyle()
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.top, 8)
                    HStack(spacing: 8) {
                        WhereTag(backend: space?.backend ?? (key.session == HerdrSession.herdrApp ? "hc" : "herdr"))
                        if space?.fromSwitchboard == true { Text("Started from Switchboard").font(Face.sub).foregroundStyle(Ink.sub) }
                    }
                    .padding(.top, 12)
                    if let space {
                        if !space.live.isEmpty {
                            SectionHead("Agents", count: space.live.count)
                            ForEach(Array(space.live.enumerated()), id: \.element) { i, a in row(a, first: i == 0) }
                        }
                        if !space.ended.isEmpty {
                            SectionHead("Ended today", count: space.ended.count)
                            ForEach(Array(space.ended.enumerated()), id: \.element) { i, a in row(a, first: i == 0) }
                        }
                        if space.agents.isEmpty { StateRow(text: "No agents in this space.") }
                        if space.newAgent == .yes {
                            Button { starting = true } label: {
                                Row(title: "New agent here", subtitle: "A new session, in this space", first: space.agents.isEmpty) {
                                    Image(systemName: "plus").font(.body.weight(.medium)).foregroundStyle(Ink.ac).frame(width: 22)
                                } trailing: { Chevron() }
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("newAgentHere")
                        }
                    } else {
                        StateRow(text: "No agents in this space.")
                    }
                    SectionHead("At your desk")
                    let app = key.session == HerdrSession.herdrApp
                    FieldRow("Where", value: "\(app ? "Herdr app" : "Herdr terminal") on \(key.machine)", first: true)
                    if app {
                        FieldRow("Find it under", value: key.space.map { "the space “\($0)”" } ?? "no space")
                    } else {
                        if let ws = key.space { FieldRow("Workspace", value: ws) }
                        FieldRow(label: "Attach") {
                            Text("herdr session attach \(key.session)").font(Face.mono(12.5)).textSelection(.enabled)
                        }
                    }
                    if let space {
                        NameRow(name: key.space ?? space.title, offer: space.rename) { renaming = true }
                        if case .no(let why) = space.newAgent {
                            Hairline()
                            Note(text: why, color: Ink.sub).padding(.vertical, 11).accessibilityIdentifier("newAgentBlocked")
                        }
                    }
                    Hairline()
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 32)
            }
            .refreshable { await model.refreshAgents() }
        }
        .inboxPage()
        .sheet(isPresented: $renaming) {
            let name = key.space ?? space?.title ?? ""
            RenameSheet(title: "Rename \(name)", current: name) { try await rename(to: $0) }
        }
        .sheet(isPresented: $starting) {
            if let space, let id = space.spaceId {
                StartSheet(project: space.project, into: .init(spaceId: id, title: space.title, machine: key.machine)) { _ in
                    starting = false
                    Task { await model.refreshAgents() }
                }
            }
        }
        .task(id: model.followKey) {
            while model.followKey != nil, !Task.isCancelled {
                await model.refreshAgents()
                try? await Task.sleep(for: .seconds(6))
            }
        }
    }

    private func rename(to name: String) async throws {
        guard let hub = model.hub, let id = space?.spaceId else { return }
        let requestId = renameIds[name] ?? UUID().uuidString
        renameIds[name] = requestId
        let listed = try await hub.renameSpace(machine: key.machine, herdrSession: key.session, spaceId: id, name: name,
                                               requestId: requestId)
        await model.refreshAgents()
        replaceTop?(.space(.init(machine: key.machine, session: key.session, space: listed)))
    }

    @ViewBuilder
    private func row(_ a: HerdrSession.Agent, first: Bool) -> some View {
        let status = a.status
        AgentLink(agent: a) {
            Row(title: a.nameInSpace, subtitle: a.subtitle,
                subtitleColor: status.glyph == .need ? Ink.warn : Ink.sub, dim: a.isEnded, first: first) {
                StatusGlyph(glyph: a.isEnded ? .end : status.glyph)
            } trailing: {
                if case .ended = a { EmptyView() } else { Chevron() }
            }
        }
        .buttonStyle(.plain)
    }
}
