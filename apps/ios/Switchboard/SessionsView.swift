import SwiftUI
import SwitchboardKit

struct SessionsView: View {
    @Environment(AppModel.self) private var model
    @State private var path: [String] = []
    @State private var starting = false
    @State private var showingDevices = false

    var body: some View {
        NavigationStack(path: $path) {
            List {
                if !model.connected {
                    Label("Can't reach the hub; retrying.", systemImage: "wifi.exclamationmark")
                        .foregroundStyle(.orange)
                }
                let waiting = model.sessions.filter { !$0.pendingApprovals.isEmpty }
                if !waiting.isEmpty {
                    Section("Waiting for you") {
                        ForEach(waiting, id: \.id) { s in
                            NavigationLink(value: s.id) { SessionRow(session: s) }
                        }
                    }
                }
                Section("Sessions") {
                    if model.sessions.isEmpty {
                        Text("No sessions yet. Start one with +.").foregroundStyle(.secondary)
                    }
                    ForEach(model.sessions.filter { $0.pendingApprovals.isEmpty }, id: \.id) { s in
                        NavigationLink(value: s.id) { SessionRow(session: s) }
                    }
                }
            }
            .accessibilityIdentifier("sessionsList")
            .navigationTitle("Switchboard")
            .navigationDestination(for: String.self) { id in SessionDetailView(sessionId: id) }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { showingDevices = true } label: { Image(systemName: "iphone.gen3") }
                        .accessibilityLabel("Devices")
                        .accessibilityIdentifier("devicesButton")
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { starting = true } label: { Image(systemName: "plus") }
                        .accessibilityLabel("Start a session")
                        .accessibilityIdentifier("startButton")
                }
            }
            .sheet(isPresented: $starting) {
                StartSheet { id in
                    starting = false
                    if let id { path.append(id) }
                }
            }
            .sheet(isPresented: $showingDevices) { DevicesView() }
            .onChange(of: model.openSessionId) { _, id in
                if let id {
                    path = [id]
                    model.openSessionId = nil
                }
            }
        }
    }
}

struct SessionRow: View {
    let session: Schemas.Session

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(session.label).font(.headline)
                Spacer()
                StatusBadge(session: session)
            }
            HStack(spacing: 8) {
                Text(session.agent)
                Text(session.machineId)
                if let branch = session.branch { Text(branch).lineLimit(1) }
                if session.muted { Image(systemName: "bell.slash") }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            if let a = session.pendingApprovals.first {
                Text(a.kind == "permission" ? "Approval needed: \(a.toolName)" : "Claude is asking a question")
                    .font(.caption.bold())
                    .foregroundStyle(.orange)
            }
        }
    }
}

struct StatusBadge: View {
    let session: Schemas.Session

    var body: some View {
        let (text, color): (String, Color) = {
            if session.hookFailed { return ("stuck", .red) }
            if !session.pendingApprovals.isEmpty { return ("waiting", .orange) }
            if session.lifecycle != "running" { return (session.lifecycle, .gray) }
            if session.machineStatus != "ready" { return (session.machineStatus, .gray) }
            switch session.agentStatus {
            case "working": return ("working", .blue)
            case "blocked": return ("blocked", .orange)
            case "idle", "done": return (session.agentStatus, .green)
            default: return (session.agentStatus, .gray)
            }
        }()
        Text(text)
            .font(.caption.bold())
            .padding(.horizontal, 8)
            .padding(.vertical, 2)
            .background(color.opacity(0.15), in: Capsule())
            .foregroundStyle(color)
    }
}
