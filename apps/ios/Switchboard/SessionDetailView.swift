import SwiftUI
import SwitchboardKit

/// One session, read like a message: its state, what you said, its latest turns, what it wants
/// and what it changed. Long-polled while on screen. The mic in the toolbar talks to it.
struct SessionDetailView: View {
    @Environment(AppModel.self) private var model
    @Environment(Voice.self) private var voice
    let sessionId: String
    @State private var session: Schemas.Session?
    @State private var convo: ConversationModel
    @State private var changes: Schemas.Changes?
    /// From the last action the user took; polls don't clear it.
    @State private var error: String?
    /// The long-poll's own trouble, cleared once it gets through again.
    @State private var pollError: String?
    /// What a delete did, kept on screen since the session leaves the lists.
    @State private var notice: String?
    @State private var confirmStop = false

    private var running: Bool { session?.lifecycle == "running" }

    init(sessionId: String) {
        self.sessionId = sessionId
        _convo = State(initialValue: ConversationModel(.session(sessionId)))
    }

    var body: some View {
        ScrollViewReader { scroll in
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    if let error { Text(error).foregroundStyle(.red) }
                    if let notice {
                        Text(notice).font(.footnote).foregroundStyle(.secondary).accessibilityIdentifier("deleteNotice")
                    }
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
        .task(id: model.wakes) { await convo.follow(model) }
        .confirmationDialog(
            running
                ? "Stop keeps the worktree and branch. Delete also deletes its Herdr session, and its worktree if it's clean and merged."
                : "Delete this session? Its Herdr session is deleted, and its worktree if it's clean and merged. The branch is kept.",
            isPresented: $confirmStop, titleVisibility: .visible
        ) {
            if running { Button("Stop", role: .destructive) { Task { await stop() } } }
            Button("Delete", role: .destructive) { Task { await delete() } }
                .accessibilityIdentifier("confirmDelete")
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
        SummaryCard(convo: convo, conversation: ConversationRef(source: .session(s.id), title: s.label))
        ConversationLinks(source: .session(s.id), title: s.label, hasConversation: convo.unavailable == nil)
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
                Button { confirmStop = true } label: { Image(systemName: running ? "stop.circle" : "trash") }
                    .tint(.red)
                    .disabled(["starting", "deleted"].contains(s.lifecycle))
                    .accessibilityLabel(running ? "Stop or delete session" : "Delete session")
                    .accessibilityIdentifier("stopOrDelete")
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

    /// Long-polls until the view goes away; each change also refreshes the changed files.
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
                if changed { await loadChanges() }
            } catch {
                if Task.isCancelled { return }
                pollError = error.localizedDescription
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    private func refresh() async {
        guard let hub = model.hub else { return }
        do {
            session = try await hub.session(sessionId)
            await convo.refresh(hub)
            await loadChanges()
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func loadChanges() async {
        guard let hub = model.hub else { return }
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

    private func delete() async {
        guard let hub = model.hub else { return }
        do {
            notice = Self.describe(try await hub.delete(sessionId))
        } catch {
            self.error = error.localizedDescription
        }
        await refresh()
    }

    static func describe(_ r: Schemas.DeleteResponse) -> String {
        let branch = r.branch.map { " Branch \($0) is kept." } ?? ""
        if r.alreadyDeleted { return "Already deleted." }
        if r.worktreeRemoved { return "Deleted, with its worktree.\(branch)" }
        if let why = r.keptBecause { return "Deleted. Its worktree was kept: \(why).\(branch)" }
        return "Deleted.\(branch)"
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
