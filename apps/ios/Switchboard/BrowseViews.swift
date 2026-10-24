import SwiftUI
import SwitchboardKit

/// Every session the hub knows, running first, then the agents started in Herdr.
struct AllSessionsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let live = model.sessions.filter { ["running", "starting"].contains($0.lifecycle) }
        let ended = model.sessions.filter { !["running", "starting"].contains($0.lifecycle) }
        List {
            if !live.isEmpty { Section("Running") { ForEach(live, id: \.id) { row($0) } } }
            if !model.agents.isEmpty { Section("Started in Herdr") { ForEach(model.agents, id: \.self) { row($0) } } }
            if !ended.isEmpty { Section("Ended") { ForEach(ended, id: \.id) { row($0) } } }
            if model.sessions.isEmpty && model.agents.isEmpty { Text("No sessions yet.").foregroundStyle(.secondary) }
        }
        .navigationTitle("All sessions")
        .task { await model.refreshAgents() }
        .refreshable { await model.refreshAgents() }
    }

    private func row(_ a: Schemas.AgentView) -> some View {
        NavigationLink(value: AgentRef(machine: a.machineId, name: a.name)) {
            VStack(alignment: .leading, spacing: 2) {
                Text(a.displayName).font(.headline)
                HStack(spacing: 6) {
                    let status = SessionStatus(agentStatus: a.agentStatus)
                    Text(status.text).foregroundStyle(status.color)
                    Text("· \(a.herdrSession ?? a.machineId)").foregroundStyle(.secondary)
                }
                .font(.subheadline)
            }
        }
    }

    private func row(_ s: Schemas.Session) -> some View {
        NavigationLink(value: s.id) {
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(s.label).font(.headline)
                    Spacer()
                    Text(Ago.short(s.updatedAt)).font(.subheadline).foregroundStyle(.secondary)
                }
                HStack(spacing: 6) {
                    Text(SessionStatus(s).text).foregroundStyle(SessionStatus(s).color)
                    Text("· \(s.projectId) · \(s.machineId)").foregroundStyle(.secondary)
                }
                .font(.subheadline)
            }
        }
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
        .navigationTitle("Machines")
        .task { await load() }
        .refreshable { await load() }
    }

    private func load() async {
        guard let hub = model.hub else { return }
        do { machines = try await hub.machines(); error = nil } catch { self.error = error.localizedDescription }
        await model.refreshAgents()
    }

    /// "1 session running · 6 agents in Herdr, 5 idle"
    private func counts(_ machine: String) -> String {
        let running = model.sessions.filter { $0.machineId == machine && $0.lifecycle == "running" }.count
        let agents = model.agents.filter { $0.machineId == machine }
        var text = "\(running) \(running == 1 ? "session" : "sessions") running"
        guard !agents.isEmpty else { return text }
        text += " · \(agents.count) \(agents.count == 1 ? "agent" : "agents") in Herdr"
        let idle = agents.filter { ["idle", "done"].contains($0.agentStatus) }.count
        if idle == agents.count { text += agents.count == 1 ? ", idle" : ", all idle" } else if idle > 0 { text += ", \(idle) idle" }
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
