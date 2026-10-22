import SwiftUI
import SwitchboardKit

/// One session, read like a message: its state, what you said, what it's doing, what it wants
/// and what it changed. Long-polled while on screen. The mic in the toolbar talks to it.
struct SessionDetailView: View {
    @Environment(AppModel.self) private var model
    @Environment(Voice.self) private var voice
    let sessionId: String
    @State private var session: Schemas.Session?
    @State private var output = ""
    @State private var outputError: String?
    @State private var changes: Schemas.Changes?
    /// From the last action the user took; polls don't clear it.
    @State private var error: String?
    /// The long-poll's own trouble, cleared once it gets through again.
    @State private var pollError: String?
    @State private var confirmStop = false
    @State private var showingOutput = false

    var body: some View {
        ScrollViewReader { scroll in
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if let error { Text(error).foregroundStyle(.red) }
                    if let pollError { Text(pollError).font(.footnote).foregroundStyle(.orange) }
                    if let s = session { content(s) } else { ProgressView().frame(maxWidth: .infinity) }
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 120)
            }
            .safeAreaInset(edge: .bottom) {
                if let s = session { toolbar(s, scroll: scroll) }
            }
        }
        .background(Color(.systemGroupedBackground))
        .navigationTitle(session?.label ?? "Session")
        .navigationBarTitleDisplayMode(.large)
        .refreshable { await refresh() }
        .task(id: model.wakes) { await follow() }
        .task(id: model.wakes) { await pollOutput() }
        .sheet(isPresented: $showingOutput) { FullOutput(title: session?.label ?? "Output", text: output) }
        .confirmationDialog("Stop this session? The worktree and branch are kept.", isPresented: $confirmStop) {
            Button("Stop", role: .destructive) { Task { await stop() } }
        }
    }

    @ViewBuilder
    private func content(_ s: Schemas.Session) -> some View {
        HStack(spacing: 8) {
            StatusPill(status: SessionStatus(s))
            Text([s.agent.capitalized, s.machineId, s.branch].compactMap { $0 }.joined(separator: " · "))
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            if s.muted { Image(systemName: "bell.slash").font(.footnote).foregroundStyle(.secondary) }
        }
        .padding(.top, -6)
        if let e = s.error { Text(e).font(.footnote).foregroundStyle(.red) }
        if s.hookFailed {
            Label("It's stuck at a dialog Switchboard won't answer. Answer it in Herdr.", systemImage: "exclamationmark.triangle")
                .font(.footnote)
                .foregroundStyle(.red)
        }
        if let said = model.said[s.id] { SaidBubble(said: said) }
        ForEach(s.pendingApprovals, id: \.id) { a in
            ApprovalCard(approval: a, session: s) { await refresh() }
        }
        VStack(alignment: .leading, spacing: 8) {
            Text("Latest").font(.footnote.weight(.semibold)).foregroundStyle(.secondary).textCase(.uppercase)
            OutputBlock(text: output)
                .accessibilityIdentifier("output")
            if let outputError { Text(outputError).font(.footnote).foregroundStyle(.orange) }
            Button { showingOutput = true } label: {
                Label("Show the full output", systemImage: "chevron.right").labelStyle(TrailingIcon())
            }
            .font(.body)
        }
        if let changes, !changes.files.isEmpty {
            ChangesGrid(changes: changes).id("changes")
        }
        DisclosureGroup("Details") {
            VStack(alignment: .leading, spacing: 8) {
                LabeledContent("Project", value: s.projectId)
                LabeledContent("Machine", value: "\(s.machineId) (\(s.machineStatus))")
                if let h = s.herdrSession {
                    LabeledContent("Herdr session", value: h)
                    Text("On \(s.machineId): herdr session attach \(h)")
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
            }
            .padding(.top, 8)
        }
        .font(.subheadline)
    }

    private func toolbar(_ s: Schemas.Session, scroll: ScrollViewProxy) -> some View {
        let running = s.lifecycle == "running"
        return HStack {
            Group {
                Button { confirmStop = true } label: { Image(systemName: "stop.circle") }
                    .tint(.red)
                    .disabled(!running)
                    .accessibilityLabel("Stop session")
                Button { Task { await mute(!s.muted) } } label: { Image(systemName: s.muted ? "bell.slash" : "bell") }
                    .accessibilityLabel(s.muted ? "Turn notifications on" : "Mute notifications")
            }
            .frame(maxWidth: .infinity)
            MicButton(size: 60) { voice.listen(to: .session(s)) }
                .disabled(!running)
                .accessibilityLabel("Talk to \(s.label)")
                .accessibilityIdentifier("sessionMic")
            Group {
                Button { withAnimation { scroll.scrollTo("changes", anchor: .top) } } label: { Image(systemName: "doc.text") }
                    .disabled(changes?.files.isEmpty ?? true)
                    .accessibilityLabel("Changes")
                Button { voice.type(to: .session(s)) } label: { Image(systemName: "keyboard") }
                    .disabled(!running)
                    .accessibilityLabel("Type a message to \(s.label)")
            }
            .frame(maxWidth: .infinity)
        }
        .font(.system(size: 21))
        .padding(.horizontal, 8)
        .padding(.top, 6)
        .background(.bar)
    }

    /// Long-polls until the view goes away; each change also refreshes the output and changes.
    private func follow() async {
        pollError = nil
        var revision: Int64?
        while !Task.isCancelled {
            guard let hub = model.hub else { return }
            do {
                let s = try await hub.session(sessionId, after: revision)
                let changed = s.revision != revision
                session = s
                revision = s.revision
                pollError = nil
                if changed { await loadOutput() }
            } catch {
                if Task.isCancelled { return }
                pollError = error.localizedDescription
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    /// The agent writes without changing the session's status, so the tail is read every few
    /// seconds while it runs and the screen is open.
    private func pollOutput() async {
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(3))
            if session?.lifecycle == "running" { await loadOutput() }
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
        do {
            output = try await hub.output(sessionId, lines: 300).text
            outputError = nil
        } catch {
            if !Task.isCancelled { outputError = "Couldn't refresh the output: \(error.localizedDescription)" }
        }
        // Older runners don't list changes; the section just stays hidden.
        if let c = try? await hub.changes(sessionId) { changes = c }
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

/// The whole tail of an agent's terminal, selectable.
struct FullOutput: View {
    @Environment(\.dismiss) private var dismiss
    let title: String
    let text: String

    var body: some View {
        NavigationStack {
            ScrollView {
                Text(text.isEmpty ? "No output yet." : text)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(Color(white: 0.9))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(16)
                    .textSelection(.enabled)
            }
            .defaultScrollAnchor(.bottom)
            .background(Color.codeBackground)
            .navigationTitle(title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
    }
}

struct TrailingIcon: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 4) {
            configuration.title
            configuration.icon.font(.footnote.weight(.semibold))
        }
    }
}
