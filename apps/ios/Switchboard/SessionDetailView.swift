import SwiftUI
import SwitchboardKit

/// One session, long-polled while it's on screen: status, approvals, the output tail and actions.
struct SessionDetailView: View {
    @Environment(AppModel.self) private var model
    let sessionId: String
    @State private var session: Schemas.Session?
    @State private var output = ""
    @State private var error: String?
    @State private var prompt = ""
    @State private var confirmStop = false

    var body: some View {
        List {
            if let error { Text(error).foregroundStyle(.red) }
            if let s = session {
                Section {
                    LabeledContent("Status") { StatusBadge(session: s) }
                    LabeledContent("Project", value: s.projectId)
                    LabeledContent("Machine", value: "\(s.machineId) (\(s.machineStatus))")
                    LabeledContent("Agent", value: s.agent)
                    if let b = s.branch { LabeledContent("Branch", value: b) }
                    if let e = s.error { Text(e).font(.caption).foregroundStyle(.red) }
                    if s.hookFailed {
                        Text("The agent is stuck at a dialog Switchboard won't answer. Answer it in Herdr.")
                            .font(.caption).foregroundStyle(.red)
                    }
                }
                if !s.pendingApprovals.isEmpty {
                    Section("Waiting for you") {
                        ForEach(s.pendingApprovals, id: \.id) { a in
                            ApprovalCard(approval: a, sessionLabel: s.label) { await refresh() }
                        }
                    }
                }
                Section("Output") {
                    Text(output.isEmpty ? "No output yet." : output)
                        .font(.caption2.monospaced())
                        .textSelection(.enabled)
                        .accessibilityIdentifier("output")
                    Button("Refresh output") { Task { await loadOutput() } }
                }
                if s.lifecycle == "running" {
                    Section("Follow-up prompt") {
                        TextField("Tell the agent…", text: $prompt, axis: .vertical)
                        Button("Send") { Task { await send() } }
                            .disabled(prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }
                Section {
                    Toggle("Notifications", isOn: Binding(
                        get: { !s.muted },
                        set: { on in Task { await mute(!on) } }
                    ))
                    if s.lifecycle == "running" {
                        Button("Stop session", role: .destructive) { confirmStop = true }
                    }
                }
            } else {
                ProgressView()
            }
        }
        .navigationTitle(session?.label ?? "Session")
        .refreshable { await refresh() }
        .task { await follow() }
        .confirmationDialog("Stop this session? The worktree and branch are kept.", isPresented: $confirmStop) {
            Button("Stop", role: .destructive) { Task { await stop() } }
        }
    }

    /// Long-polls until the view goes away; each change also refreshes the output tail.
    private func follow() async {
        var revision: Int64?
        while !Task.isCancelled {
            guard let hub = model.hub else { return }
            do {
                let s = try await hub.session(sessionId, after: revision)
                let changed = s.revision != revision
                session = s
                revision = s.revision
                error = nil
                if changed { await loadOutput() }
            } catch {
                if Task.isCancelled { return }
                self.error = error.localizedDescription
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    private func refresh() async {
        guard let hub = model.hub else { return }
        do {
            session = try await hub.session(sessionId)
            await loadOutput()
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func loadOutput() async {
        guard let hub = model.hub else { return }
        if let o = try? await hub.output(sessionId, lines: 60) { output = o.text }
    }

    private func send() async {
        guard let hub = model.hub else { return }
        do {
            _ = try await hub.prompt(sessionId, text: prompt, requestId: UUID().uuidString)
            prompt = ""
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func mute(_ muted: Bool) async {
        guard let hub = model.hub else { return }
        do { _ = try await hub.mute(sessionId, muted: muted) } catch { self.error = error.localizedDescription }
        await refresh()
    }

    private func stop() async {
        guard let hub = model.hub else { return }
        do { _ = try await hub.stop(sessionId) } catch { self.error = error.localizedDescription }
        await refresh()
    }
}

/// A pending approval with its countdown. A permission can be approved (Face ID) or denied. A
/// question or plan is labelled as such and can only be denied here, because approving it would
/// leave the agent at a dialog nobody can answer from the phone.
struct ApprovalCard: View {
    @Environment(AppModel.self) private var model
    let approval: Schemas.Approval
    let sessionLabel: String
    let decided: () async -> Void
    @State private var busy = false
    @State private var result: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                switch approval.kind {
                case "question": Label("Question from Claude", systemImage: "questionmark.bubble")
                case "plan": Label("Plan for approval", systemImage: "list.bullet.clipboard")
                default: Label(approval.toolName, systemImage: "lock.shield")
                }
                Spacer()
                TimelineView(.periodic(from: .now, by: 1)) { _ in
                    Text(Countdown.text(deadline: approval.deadline, now: Countdown.nowMs()))
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                        .accessibilityLabel("Time left")
                }
            }
            .font(.headline)
            switch approval.kind {
            case "question":
                ForEach(approval.questions, id: \.self) { Text($0) }
                Text("Answer questions in Herdr for now.").font(.caption).foregroundStyle(.secondary)
            case "plan":
                Text(approval.plan ?? "").font(.caption)
                Text("Approve plans in Herdr for now.").font(.caption).foregroundStyle(.secondary)
            default:
                Text(approval.inputText)
                    .font(.caption.monospaced())
                    .textSelection(.enabled)
            }
            if let result { Text(result).font(.caption).foregroundStyle(.secondary) }
            HStack {
                if approval.kind == "permission" {
                    Button { Task { await approve() } } label: { Label("Approve", systemImage: "faceid") }
                        .buttonStyle(.borderedProminent)
                        .accessibilityIdentifier("approve-\(approval.id)")
                }
                Button("Deny", role: .destructive) { Task { await deny() } }
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("deny-\(approval.id)")
            }
            .disabled(busy)
        }
        .padding(.vertical, 4)
    }

    private func approve() async {
        guard let hub = model.hub else { return }
        busy = true
        defer { busy = false }
        do {
            let r = try await hub.approve(approval.id, reason: "Approve \(approval.toolName) in \(sessionLabel)", requestId: UUID().uuidString)
            result = describe(r)
        } catch {
            result = error.localizedDescription
        }
        await decided()
    }

    private func deny() async {
        guard let hub = model.hub else { return }
        busy = true
        defer { busy = false }
        do {
            result = describe(try await hub.deny(approval.id, requestId: UUID().uuidString))
        } catch {
            result = error.localizedDescription
        }
        await decided()
    }

    private func describe(_ r: Schemas.DecisionResponse) -> String {
        if r.replayed { return "Already decided: \(r.outcome)." }
        if r.outcome == "allow" { return r.delivery == "unknown" ? "Approved; the hub lost the connection, so it may have run." : "Approved." }
        return "Denied."
    }
}
