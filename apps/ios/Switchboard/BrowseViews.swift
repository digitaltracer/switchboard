import SwiftUI
import SwitchboardKit

/// Every Herdr session with its agents, running first and ended at the bottom; or just one
/// machine's.
struct AllSessionsView: View {
    @Environment(AppModel.self) private var model
    var machine: String?

    var body: some View {
        let groups = model.herdrSessions(machine: machine, since: nil)
        List {
            ForEach(groups) { g in
                NavigationLink(value: g.key) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(g.key.name).font(.headline).foregroundStyle(g.isEnded ? .secondary : .primary)
                        Text(machine == nil ? "\(g.summary) · \(g.key.machine)" : g.summary)
                            .font(.subheadline)
                            .foregroundStyle(g.isEnded ? .secondary : g.status.color)
                    }
                }
                .listRowBackground(g.isEnded ? Color.endedRow : nil)
            }
            if groups.isEmpty { Text("No sessions yet.").foregroundStyle(.secondary) }
        }
        .navigationTitle(machine ?? "All sessions")
        .task { await model.refreshAgents() }
        .refreshable { await model.refreshAgents() }
    }
}

struct MachinesView: View {
    @Environment(AppModel.self) private var model
    @State private var machines: [Schemas.Machine] = []
    @State private var error: String?

    var body: some View {
        List {
            if let error { Text(error).foregroundStyle(.red) }
            ForEach(machines, id: \.id) { m in
                NavigationLink(value: Browse.machine(m.id)) {
                    HStack {
                        Image(systemName: "desktopcomputer").foregroundStyle(Color.accentColor)
                        VStack(alignment: .leading, spacing: 2) {
                            HStack(spacing: 6) {
                                Text(m.id).font(.headline)
                                if m._default { Text("default").font(.caption).foregroundStyle(.secondary) }
                            }
                            Text(counts(m.id)).font(.subheadline).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Text(m.status).font(.subheadline).foregroundStyle(m.status == "ready" ? Color.done : .secondary)
                    }
                }
            }
        }
        .navigationTitle("Machines")
        .task { await load() }
        .refreshable { await load() }
    }

    private func load() async {
        guard let hub = model.hub else { return }
        do { machines = try await hub.machines(); error = nil } catch { self.error = error.localizedDescription }
        await model.refreshAgents()
    }

    /// "2 sessions · 5 agents, 1 working"
    private func counts(_ machine: String) -> String {
        let live = model.herdrSessions(machine: machine, since: nil).filter { !$0.isEnded }
        let agents = live.flatMap(\.live)
        var text = "\(live.count) \(live.count == 1 ? "session" : "sessions")"
        guard !agents.isEmpty else { return text }
        text += " · \(agents.count) \(agents.count == 1 ? "agent" : "agents")"
        let working = agents.filter { $0.agentStatus == "working" }.count
        text += working > 0 ? ", \(working) working" : (agents.count == 1 ? ", idle" : ", all idle")
        return text
    }
}

struct ProjectsView: View {
    @Environment(AppModel.self) private var model
    @State private var projects: [Schemas.Project] = []
    @State private var error: String?

    var body: some View {
        List {
            if let error { Text(error).foregroundStyle(.red) }
            ForEach(projects, id: \.id) { p in
                VStack(alignment: .leading, spacing: 2) {
                    HStack {
                        Text(p.id).font(.headline)
                        Spacer()
                        let n = model.sessions.filter { $0.projectId == p.id && $0.lifecycle == "running" }.count
                        if n > 0 { Text("\(n) running").font(.subheadline).foregroundStyle(Color.working) }
                    }
                    if !p.description.isEmpty { Text(p.description).font(.subheadline).foregroundStyle(.secondary) }
                    Text("\(p.defaultAgent.capitalized) · \(p.machines.joined(separator: ", "))")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .navigationTitle("Projects")
        .task { await load() }
        .refreshable { await load() }
    }

    private func load() async {
        guard let hub = model.hub else { return }
        do { projects = try await hub.projects(); error = nil } catch { self.error = error.localizedDescription }
    }
}
