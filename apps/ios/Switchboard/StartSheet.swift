import SwiftUI
import SwitchboardKit

/// Starts a session. A lost answer becomes "outcome unknown" and is reconciled by request id;
/// the same start within a minute comes back as "already running".
struct StartSheet: View {
    @Environment(AppModel.self) private var model
    let done: (String?) -> Void
    @State private var projects: [Schemas.Project] = []
    @State private var machines: [Schemas.Machine] = []
    @State private var project = ""
    @State private var machine = ""
    @State private var agent = ""
    @State private var prompt = ""
    @State private var notify = true
    @State private var state: StartState?
    @State private var loadError: String?
    @State private var sending = false

    var body: some View {
        NavigationStack {
            Form {
                if let loadError { Text(loadError).foregroundStyle(.red) }
                Picker("Project", selection: $project) {
                    ForEach(projects, id: \.id) { Text($0.id).tag($0.id) }
                }
                .accessibilityIdentifier("projectPicker")
                .onChange(of: project) { _, id in
                    agent = projects.first { $0.id == id }?.defaultAgent ?? "claude"
                    let available = projects.first { $0.id == id }?.machines ?? []
                    if !available.contains(machine) { machine = available.first ?? "" }
                }
                Picker("Machine", selection: $machine) {
                    ForEach(machines.filter { m in projects.first { $0.id == project }?.machines.contains(m.id) ?? true }, id: \.id) { m in
                        Text("\(m.id) (\(m.status))").tag(m.id)
                    }
                }
                Picker("Agent", selection: $agent) {
                    Text("Claude Code").tag("claude")
                    Text("Codex").tag("codex")
                }
                Section("Prompt") {
                    TextField("What should the agent do?", text: $prompt, axis: .vertical)
                        .lineLimit(4...10)
                        .accessibilityIdentifier("promptField")
                }
                Toggle("Notify me", isOn: $notify)
                if let state { StartStatus(state: state, open: done) }
            }
            .navigationTitle("New session")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { done(nil) } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Start") { Task { await start() } }
                        .disabled(project.isEmpty || prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || sending)
                        .accessibilityIdentifier("submitStart")
                }
            }
            .task { await load() }
        }
    }

    private func load() async {
        guard let hub = model.hub else { return }
        do {
            projects = try await hub.projects()
            machines = try await hub.machines()
            if project.isEmpty, let first = projects.first { project = first.id }
            machine = machines.first(where: \._default)?.id ?? machines.first?.id ?? ""
        } catch {
            loadError = error.localizedDescription
        }
    }

    private func start() async {
        guard let hub = model.hub else { return }
        sending = true
        defer { sending = false }
        let requestId = UUID().uuidString
        state = await StartFlow.run(hub: hub, project: project, prompt: prompt, machine: machine.isEmpty ? nil : machine,
                                    agent: agent, notify: notify, requestId: requestId)
        // Reconcile an unknown outcome by asking the hub about the request, for up to five minutes.
        var tries = 0
        while case .unknown = state, tries < 150, !Task.isCancelled {
            try? await Task.sleep(for: .seconds(2))
            state = await StartFlow.reconcile(hub: hub, requestId: requestId)
            tries += 1
        }
        if case .started(let id, false) = state { done(id) }
    }
}

struct StartStatus: View {
    let state: StartState
    let open: (String?) -> Void

    var body: some View {
        switch state {
        case .started(let id, true):
            VStack(alignment: .leading) {
                Text("Already running: the same start was sent in the last minute.")
                Button("Open it") { open(id) }
            }
        case .started:
            Text("Started.")
        case .unknown:
            Label("Outcome unknown; reconciling with the hub…", systemImage: "arrow.triangle.2.circlepath")
                .accessibilityIdentifier("reconciling")
        case .failed(let message):
            Text(message).foregroundStyle(.red)
        }
    }
}
