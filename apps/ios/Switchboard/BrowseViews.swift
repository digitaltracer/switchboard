import SwiftUI
import SwitchboardKit

enum Browse: Hashable {
    case machines
    case machine(String)
}

/// One machine's spaces.
struct MachineSpacesPage: View {
    @Environment(AppModel.self) private var model
    let machine: String

    var body: some View {
        let live = model.spaces(machine: machine, since: nil).filter { !$0.isEnded }.count
        VStack(spacing: 0) {
            PageTop(back: "Back to machines", meta: nil)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    Headline("\(machine).", live == 0 ? "Nothing live." : "\(live) \(live == 1 ? "space" : "spaces") live.")
                        .stateStyle()
                        .padding(.top, 8)
                        .padding(.bottom, 14)
                    SpacesList(machine: machine)
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 32)
            }
            .refreshable { await model.refreshAgents() }
        }
        .inboxPage()
        .task { await model.refreshAgents() }
    }
}

/// Every machine with its readiness and what runs there. A session always stays on the machine
/// it started on.
struct MachinesPage: View {
    @Environment(AppModel.self) private var model
    @State private var machines: [Schemas.Machine]?
    @State private var error: String?

    var body: some View {
        VStack(spacing: 0) {
            PageTop(back: "Back to spaces", meta: nil)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    headline.stateStyle().fixedSize(horizontal: false, vertical: true).padding(.top, 8)
                    Text("Choose one to see only its spaces.").font(Face.body).foregroundStyle(Ink.sub).padding(.top, 6)
                    VStack(spacing: 0) {
                        if let error { StateRow(text: "Couldn't list the machines: \(error). Pull down to try again.", error: true) }
                        if machines == nil && error == nil { StateRow(text: "Listing machines…", busy: true) }
                        ForEach(Array((machines ?? []).enumerated()), id: \.element.id) { i, m in
                            NavigationLink(value: Browse.machine(m.id)) { row(m, first: i == 0) }
                                .buttonStyle(.plain)
                                .accessibilityIdentifier("machine-\(m.id)")
                        }
                        Hairline()
                    }
                    .padding(.top, 20)
                    Note(text: "A session always stays on the machine it started on.").padding(.top, 16)
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 32)
            }
            .refreshable { await load() }
        }
        .inboxPage()
        .task { await load() }
    }

    private var headline: Headline {
        guard let machines else { return Headline("Machines.") }
        let ready = machines.filter { $0.status == "ready" }.count
        let n = machines.count
        let quiet = ready == n ? (n == 1 ? "It's ready." : n == 2 ? "Both ready." : "All ready.") : "\(n - ready) not ready."
        return Headline("\(n) \(n == 1 ? "machine" : "machines").", quiet)
    }

    private func row(_ m: Schemas.Machine, first: Bool) -> some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    Text(m.id).font(Face.title).foregroundStyle(Ink.ink)
                    if m._default { Chip(text: "default").scaleEffect(0.86, anchor: .leading) }
                }
                Text(counts(m.id)).font(Face.sub).foregroundStyle(Ink.sub)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 6) {
                Circle().fill(m.status == "ready" ? Ink.ok : Ink.warn).frame(width: 8, height: 8)
                Text(m.status)
            }
            .font(Face.geist(13, .medium))
            .foregroundStyle(m.status == "ready" ? Ink.ok : Ink.warn)
            .padding(.top, 3)
            Chevron().padding(.top, 6)
        }
        .padding(.vertical, 18)
        .overlay(alignment: .top) { if !first { Hairline() } }
        .contentShape(Rectangle())
    }

    private func load() async {
        guard let hub = model.hub else { return }
        do { machines = try await hub.machines(); error = nil } catch { self.error = error.localizedDescription }
        await model.refreshAgents()
    }

    /// "2 spaces · 13 agents, 3 working"
    private func counts(_ machine: String) -> String {
        let live = model.spaces(machine: machine, since: nil).filter { !$0.isEnded }
        guard !live.isEmpty else { return "No spaces right now" }
        let agents = live.flatMap(\.live)
        var text = "\(live.count) \(live.count == 1 ? "space" : "spaces") · \(agents.count) \(agents.count == 1 ? "agent" : "agents")"
        let working = agents.filter { $0.agentStatus == "working" }.count
        text += working > 0 ? ", \(working) working" : (agents.count == 1 ? ", idle" : ", all idle")
        return text
    }
}

/// The allowlisted projects; tapping one starts a session there.
struct ProjectsList: View {
    @Environment(AppModel.self) private var model
    let start: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if model.projects.isEmpty { StateRow(text: "Listing projects…", busy: true) }
            ForEach(Array(model.projects.enumerated()), id: \.element.id) { i, p in
                Button { start(p.id) } label: {
                    HStack(spacing: 12) {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(p.id).font(Face.title).foregroundStyle(Ink.ink)
                            if !p.description.isEmpty {
                                Text(p.description).font(Face.sub).foregroundStyle(Ink.sub).lineLimit(1)
                            }
                            Text("\(p.defaultAgent == "codex" ? "Codex" : "Claude Code") · \(p.machines.joined(separator: ", "))")
                                .font(Face.sub).foregroundStyle(Ink.sub).lineLimit(1)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        let n = model.sessions.filter { $0.projectId == p.id && $0.lifecycle == "running" }.count
                        if n > 0 { Chip(text: "\(n) running") } else { Chevron() }
                    }
                    .padding(.vertical, 12)
                    .frame(minHeight: 60)
                    .overlay(alignment: .top) { if i > 0 { Hairline() } }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("project-\(p.id)")
            }
            Hairline()
            Note(text: "Only projects on the hub’s allowlist appear here.").padding(.top, 14)
        }
    }
}
