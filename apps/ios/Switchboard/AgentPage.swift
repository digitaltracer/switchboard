import SwiftUI
import SwitchboardKit

/// An agent as the navigation stack opens it: one Switchboard didn't start.
struct AgentRef: Hashable {
    let machine: String
    let name: String
}

/// An agent's page opened on a given tab.
struct AgentPageRef: Hashable {
    let source: ConversationSource
    var tab: AgentTab = .summary
}

enum AgentTab: Hashable { case summary, conversation, screen }

/// One agent: Summary, Conversation and Screen as tabs. A session Switchboard started adds its
/// approvals, changed files, details and a row of actions; one it didn't start has a message bar.
struct AgentPage: View {
    @Environment(AppModel.self) private var model
    @Environment(Voice.self) private var voice
    let source: ConversationSource
    @State private var tab: AgentTab
    @State private var convo: ConversationModel
    @State private var watch: DialogWatch?
    @State private var session: Schemas.Session?
    @State private var changes: Schemas.Changes?
    /// From the last action the user took; polls don't clear it.
    @State private var error: String?
    /// The long-poll's own trouble, cleared once it gets through again.
    @State private var pollError: String?
    /// What a delete did, kept on screen since the session leaves the lists.
    @State private var notice: String?
    @State private var lifecycle = false
    @State private var details = false
    @State private var desk = false
    @State private var renaming = false
    /// One request id per name tried, so a retry after a lost answer is the same request.
    @State private var renameIds: [String: String] = [:]
    @Environment(\.replaceTop) private var replaceTop

    init(source: ConversationSource, tab: AgentTab = .summary) {
        self.source = source
        _tab = State(initialValue: tab)
        _convo = State(initialValue: ConversationModel(source))
        if case .agent(let machine, let name) = source {
            _watch = State(initialValue: DialogWatch(machine: machine, agent: name))
        }
    }

    private var sessionId: String? { if case .session(let id) = source { id } else { nil } }
    private var agentRef: (machine: String, name: String)? {
        if case .agent(let m, let n) = source { (m, n) } else { nil }
    }
    private var info: Schemas.AgentView? {
        guard let a = agentRef else { return nil }
        return model.agents.first { $0.machineId == a.machine && $0.name == a.name }
    }
    private var machine: String { session?.machineId ?? agentRef?.machine ?? "" }
    private var backend: String { session?.backend ?? info?.backend ?? "herdr" }
    private var space: AgentSpace? {
        let spaces = model.spaces(since: HerdrSession.startOfToday())
        if let id = sessionId { return spaces.first { if case .task(let t) = $0 { t.id == id } else { false } } }
        guard let a = agentRef else { return nil }
        return spaces.first { if case .live(let v) = $0 { v.machineId == a.machine && v.name == a.name } else { false } }
    }
    private var title: String {
        if let s = session { return s.label }
        guard let a = agentRef else { return "Session" }
        if let space, let me = space.live.first(where: { if case .live(let v) = $0 { v.name == a.name } else { false } }) {
            return space.label(me)
        }
        return info?.displayName ?? a.name
    }
    private var kind: String? {
        let k = session?.agent ?? info?.kind
        return k.map { $0 == "claude" ? "Claude Code" : $0 == "codex" ? "Codex" : $0.capitalized }
    }
    private var target: Schemas.TargetView? {
        if let s = session { return .session(s) }
        guard let a = agentRef else { return nil }
        return .agent(machine: a.machine, name: a.name, label: title)
    }
    private var status: SessionStatus? {
        if let target, model.awaiting[AppModel.saidKey(for: target)] != nil { return .sent }
        if let s = session { return SessionStatus(s) }
        return info.map { SessionStatus(agentStatus: $0.agentStatus) }
    }
    private var running: Bool { session?.lifecycle == "running" }
    /// The agent's pane, for its dialog.
    private var pane: DecisionRef? {
        let meta = [kind, machine].compactMap { $0 }.joined(separator: " · ")
        if let s = session {
            guard s.lifecycle == "running", let name = s.agentName else { return nil }
            return DecisionRef(machine: s.machineId, agent: name, label: s.label, meta: meta)
        }
        guard let a = agentRef else { return nil }
        return DecisionRef(machine: a.machine, agent: a.name, label: title, meta: meta)
    }

    var body: some View {
        VStack(spacing: 0) {
            PageTop(back: "Back", meta: tab == .summary ? [kind, machine].compactMap { $0 }.joined(separator: " · ") : nil)
                .overlay(alignment: .leading) {
                    if tab != .summary {
                        HStack(spacing: 8) {
                            Text(title).font(Face.geist(15, .medium)).foregroundStyle(Ink.ink).lineLimit(1)
                            Spacer(minLength: 4)
                            if let status { StatusChip(status: status) }
                        }
                        .padding(.leading, 52)
                        .padding(.trailing, 14)
                    }
                }
            if tab == .summary { header }
            TextTabs(tabs: [(.summary, "Summary"), (.conversation, "Conversation"), (.screen, "Screen")], selection: $tab.animation(.snappy))
                .padding(.top, tab == .summary ? 8 : 0)
            TabView(selection: $tab) {
                summary.tag(AgentTab.summary)
                ConversationFeed(convo: convo, speaker: kind ?? "Agent", status: status).tag(AgentTab.conversation)
                screen.tag(AgentTab.screen)
            }
            .tabViewStyle(.page(indexDisplayMode: .never))
            bottom
        }
        .inboxPage()
        .onChange(of: tab, initial: true) { _, t in convo.showTerminal(t == .screen) }
        .task(id: model.followKey) { await convo.follow(model) }
        .task(id: model.followKey) { await follow() }
        .task(id: model.followKey) { if let watch { await watch.follow(model) } }
        .task(id: "\(model.followKey ?? -1)/\(pane?.agent ?? "")") {
            guard session != nil, let pane else { sessionWatch = nil; return }
            let w = DialogWatch(machine: pane.machine, agent: pane.agent)
            sessionWatch = w
            await w.follow(model)
        }
        // Its status chip: only home refreshes agents otherwise.
        .task(id: model.followKey) {
            guard agentRef != nil else { return }
            while model.followKey != nil, !Task.isCancelled {
                await model.refreshAgents()
                try? await Task.sleep(for: .seconds(5))
            }
        }
        .sheet(isPresented: $lifecycle) { lifecycleSheet }
        .sheet(isPresented: $renaming) {
            RenameSheet(title: "Rename \(ownName)", current: ownName, herdrAgent: me?.isHerdrAgent ?? false) {
                try await rename(to: $0)
            }
        }
    }

    /// This page's agent as the lists have it.
    private var me: HerdrSession.Agent? {
        if let s = session { return .task(s) }
        return info.map { .live($0) }
    }
    /// The name a rename changes: a session's label, or an agent's own name without its space.
    private var ownName: String { session?.label ?? info?.agentLabel ?? title }

    private func rename(to name: String) async throws {
        guard let hub = model.hub else { return }
        if let id = sessionId {
            _ = try await hub.renameSession(id, name: name)
            await refresh()
            if let s = session { model.renamed(session: s) }
            return
        }
        guard let a = agentRef else { return }
        let requestId = renameIds[name] ?? UUID().uuidString
        renameIds[name] = requestId
        let listed = try await hub.renameAgent(machine: a.machine, agent: a.name, name: name, requestId: requestId)
        model.renamed(agent: a.name, to: listed, machine: a.machine)
        await model.refreshAgents()
        replaceTop?(.agent(AgentRef(machine: a.machine, name: listed)))
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title)
                .font(Face.h1)
                .tracking(-0.84)
                .foregroundStyle(Ink.ink)
                .lineLimit(2)
                .accessibilityAddTraits(.isHeader)
            HStack(spacing: 8) {
                if let status { StatusChip(status: status) }
                WhereTag(backend: backend)
                if let branch = session?.branch {
                    Text("branch \(branch)").font(Face.mono(12.5)).foregroundStyle(Ink.sub).lineLimit(1)
                }
                if session?.muted == true {
                    Image(systemName: "bell.slash").font(.footnote).foregroundStyle(Ink.mut).accessibilityLabel("Muted")
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // Summary.

    private var summary: some View {
        ScrollViewReader { scroll in
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    if let error { Note(text: error, color: Ink.bad).padding(.top, 14) }
                    if let notice { Note(text: notice, color: Ink.sub).padding(.top, 14).accessibilityIdentifier("deleteNotice") }
                    if let pollError { Note(text: pollError, color: Ink.warn).padding(.top, 14) }
                    if let s = session {
                        if let e = s.error { Note(text: e, color: Ink.bad).padding(.top, 14) }
                        if s.hookFailed {
                            Banner(text: "It's stuck at a dialog Switchboard won't answer. Answer it at its terminal.").padding(.top, 14)
                        }
                    }
                    if let act = model.away, let mine = model.away(for: machine, agent: session?.agentName ?? agentRef?.name, session: sessionId) {
                        AwayTimeline(agent: mine, since: act.since, now: act.now, reply: replyShown ? nil : mine.lastReply)
                    }
                    if let target, let said = model.said[AppModel.saidKey(for: target)] {
                        SaidBubble(said: said).padding(.top, 14)
                    }
                    if let s = session {
                        ForEach(s.pendingApprovals, id: \.id) { a in
                            ApprovalCard(approval: a, session: s) { await refresh() }.padding(.top, 14)
                        }
                    }
                    if let pane, let w = watch ?? sessionWatch {
                        WaitingCard(watch: w, ref: pane) { tab = .screen }.padding(.top, 14)
                    }
                    if let t = info?.shownTitle {
                        SectionHead("Working on")
                        Text(t).font(Face.geist(20, .medium)).tracking(-0.4).foregroundStyle(Ink.ink)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    latestReply
                    if let changes, !changes.files.isEmpty {
                        VStack(spacing: 0) {
                            SectionHead(title: changesTitle(changes)) {
                                if let sessionId {
                                    NavigationLink(value: DiffRef(sessionId: sessionId, paths: changes.files.map(\.path), index: 0)) {
                                        Text("Review all").font(Face.geist(13, .medium)).foregroundStyle(Ink.ac).textCase(nil)
                                    }
                                    .accessibilityIdentifier("reviewAll")
                                }
                            }
                            ChangesList(changes: changes, sessionId: sessionId)
                                .padding(.horizontal, 14)
                                .background(Ink.sf, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                                .overlay { RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Ink.line) }
                        }
                        .id("changes")
                    }
                    if let t = shipTarget {
                        ShipCard(target: t, project: session?.projectId, branch: session?.branch, status: currentStatus,
                                 reply: Turns.latest(convo.conversation.items).reply?.text,
                                 replyAt: Turns.latest(convo.conversation.items).at.flatMap(Turns.date))
                    }
                    conversationRows
                    if session != nil {
                        SectionHead(title: "Details") {
                            Button(details ? "Hide" : "Show") { withAnimation(.snappy) { details.toggle() } }
                                .font(Face.geist(13, .medium)).foregroundStyle(Ink.ac)
                        }
                    }
                    if details, let s = session { sessionDetails(s) }
                    atYourDesk
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 24)
            }
            .refreshable { await refresh() }
            .onChange(of: scrollToChanges) { _, go in
                if go { withAnimation { scroll.scrollTo("changes", anchor: .top) }; scrollToChanges = false }
            }
        }
    }

    @State private var scrollToChanges = false
    /// A session's own agent's dialog, watched once its pane is known.
    @State private var sessionWatch: DialogWatch?

    /// "4 files changed · +86 −12".
    private func changesTitle(_ c: Schemas.Changes) -> String {
        let added = c.files.compactMap(\.added).reduce(0, +)
        let deleted = c.files.compactMap(\.deleted).reduce(0, +)
        return "\(ChangesList(changes: c).title) · +\(added) −\(deleted)"
    }

    /// Who a ship button goes to: the session, or an agent Switchboard didn't start.
    private var shipTarget: Schemas.TargetView? {
        if let s = session { return .session(s) }
        if let a = agentRef { return .agent(machine: a.machine, name: a.name, label: info?.agentLabel) }
        return nil
    }

    private var currentStatus: String { session?.agentStatus ?? info?.agentStatus ?? "unknown" }

    /// Whether "Latest reply" shows, or is about to show, the agent's own reply.
    private var replyShown: Bool {
        !convo.ended && convo.unavailable == nil
            && (!convo.conversation.isLoaded || Turns.latest(convo.conversation.items).reply != nil)
    }

    @ViewBuilder
    private var latestReply: some View {
        let latest = Turns.latest(convo.conversation.items)
        SectionHead(title: "Latest reply") {
            if let at = latest.at.flatMap(Turns.time) { Text(at) }
        }
        Group {
            if convo.ended {
                Note(text: "This session has ended.", color: Ink.sub)
            } else if let why = convo.unavailable {
                Note(text: "No conversation to show (\(why)). Its screen is under Screen.", color: Ink.sub)
            } else if !convo.conversation.isLoaded {
                ConversationSkeleton(rows: 1)
            } else if let reply = latest.reply?.text {
                Clamped(height: 168) { MarkdownText(text: reply) }
                    .contentShape(Rectangle())
                    .onTapGesture { tab = .conversation }
            } else {
                Note(text: "Nothing said yet.", color: Ink.sub)
            }
        }
        .accessibilityIdentifier("latest")
        // A session lists its changed files right below, so the files chip would only repeat it.
        let tags = changes?.files.isEmpty == false ? latest.tags.filter { !$0.text.contains("file") } : latest.tags
        if !tags.isEmpty { TagRow(tags: tags).padding(.top, 14) }
    }

    @ViewBuilder
    private var conversationRows: some View {
        VStack(spacing: 0) {
            if convo.unavailable == nil && !convo.ended {
                Button { tab = .conversation } label: {
                    Row(title: "Read the whole conversation", leading: { EmptyView() }, trailing: { Chevron() })
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("openConversation")
            }
            if !convo.ended {
                Button { tab = .screen } label: {
                    Row(title: "See its terminal", leading: { EmptyView() }, trailing: { Chevron() })
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("openScreen")
            }
            Hairline()
        }
        .padding(.top, 18)
    }

    @ViewBuilder
    private func sessionDetails(_ s: Schemas.Session) -> some View {
        VStack(spacing: 0) {
            FieldRow("Project", value: s.projectId, first: true)
            FieldRow(label: "Machine") {
                HStack(spacing: 8) {
                    Circle().fill(s.machineStatus == "ready" ? Ink.ok : Ink.warn).frame(width: 8, height: 8)
                    Text("\(s.machineId) · \(s.machineStatus)")
                }
            }
            if let b = s.branch { FieldRow("Branch", value: b, mono: true) }
            Hairline()
        }
        .padding(.top, 8)
    }

    @ViewBuilder
    private var atYourDesk: some View {
        let app = backend == "hc"
        SectionHead(title: "At your desk") {
            Button(desk ? "Hide" : "Show") { withAnimation(.snappy) { desk.toggle() } }
                .font(Face.geist(13, .medium)).foregroundStyle(Ink.ac)
                .accessibilityIdentifier("deskToggle")
        }
        if desk { VStack(spacing: 0) {
            FieldRow(app ? "Where" : "Where", value: "\(app ? "Herdr app" : "Herdr terminal") on \(machine)", first: true)
            if app {
                if let name = space?.key.space { FieldRow("Space", value: name) }
            } else if session.map({ ["starting", "running", "stopping"].contains($0.lifecycle) }) ?? true,
                      let attach = space?.attach ?? session?.herdrSession ?? info?.herdrSession {
                if let ws = space?.key.space { FieldRow("Workspace", value: ws) }
                FieldRow(label: "Attach") {
                    Text("herdr session attach \(attach)").font(Face.mono(12.5)).textSelection(.enabled)
                }
            }
            if let me { NameRow(name: ownName, offer: me.rename) { renaming = true } }
            Hairline()
        } }
    }

    // Screen.

    @ViewBuilder
    private var screen: some View {
        Group {
            if convo.terminalLoaded {
                TerminalView(text: convo.terminal, ansi: convo.terminalAnsi)
            } else if let error = convo.terminalError {
                Waiting(symbol: "exclamationmark.triangle", text: "Couldn't read the screen: \(error)", busy: false)
            } else if convo.ended {
                Waiting(symbol: "stop.circle", text: "This session has ended.", busy: false)
            } else {
                Waiting(symbol: "terminal", text: "Loading the screen…", busy: true)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Ink.term)
    }

    // The bottom: actions for a session's summary, else the message bar.

    @ViewBuilder
    private var bottom: some View {
        if let s = session, tab == .summary {
            actions(s)
        } else {
            AskBar(placeholder: "Message \(session?.label ?? info?.agentLabel ?? title), or / for skills", dark: tab == .screen,
                   type: { voice.type(to: target) }, speak: { voice.listen(to: target) })
                .disabled(target == nil || (session != nil && !running))
        }
    }

    private func actions(_ s: Schemas.Session) -> some View {
        HStack {
            ActionButton(symbol: running ? "stop.circle" : "trash", label: running ? "Stop" : "Delete", destructive: true) {
                lifecycle = true
            }
            .disabled(["starting", "deleted"].contains(s.lifecycle))
            .accessibilityLabel(running ? "Stop or delete session" : "Delete session")
            .accessibilityIdentifier("stopOrDelete")
            Spacer()
            ActionButton(symbol: s.muted ? "bell.slash" : "bell", label: s.muted ? "Muted" : "Notify") {
                Task { await mute(!s.muted) }
            }
            .accessibilityLabel(s.muted ? "Turn notifications on" : "Mute notifications")
            Spacer()
            Button { voice.listen(to: .session(s)) } label: { MicDisc(size: 52) }
                .buttonStyle(.plain)
                .disabled(!running)
                .opacity(running ? 1 : 0.4)
                .accessibilityLabel("Talk to \(s.label)")
                .accessibilityIdentifier("sessionMic")
            Spacer()
            ActionButton(symbol: "doc.text", label: "Changes") { scrollToChanges = true }
                .disabled(changes?.files.isEmpty ?? true)
                .accessibilityLabel("Changes")
            Spacer()
            ActionButton(symbol: "keyboard", label: "Type") { voice.type(to: .session(s)) }
                .disabled(!running)
                .accessibilityLabel("Type a message to \(s.label)")
        }
        .padding(.horizontal, 20)
        .padding(.top, 8)
        .padding(.bottom, 6)
        .background(Ink.bg)
        .overlay(alignment: .top) { Hairline() }
    }

    @ViewBuilder
    private var lifecycleSheet: some View {
        let name = session?.label ?? "this session"
        if running {
            ChoiceSheet(title: "Stop or delete \(name)?", choices: [
                .init(title: "Stop", detail: "Ends the agent. The worktree and branch stay.") { Task { await stop() } },
                .init(title: "Delete", detail: "Also removes its Herdr session. The worktree goes only if it is clean and merged. The branch is always kept.") {
                    Task { await delete() }
                },
            ])
        } else {
            ChoiceSheet(title: "Delete \(name)?", choices: [
                .init(title: "Delete", detail: "Removes its Herdr session. The worktree goes only if it is clean and merged. The branch is always kept.") {
                    Task { await delete() }
                },
            ])
        }
    }

    // Following a session.

    /// Long-polls until the view goes away; each change also refreshes the changed files.
    private func follow() async {
        guard let sessionId else { return }
        pollError = nil
        var revision: Int64?
        while model.followKey != nil, !Task.isCancelled {
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
        await convo.refresh(hub)
        guard let sessionId else { return }
        do {
            session = try await hub.session(sessionId)
            await loadChanges()
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func loadChanges() async {
        guard let hub = model.hub, let sessionId else { return }
        // Older runners don't list changes; the section just stays hidden.
        if let c = try? await hub.changes(sessionId) { changes = c }
    }

    private func mute(_ muted: Bool) async {
        guard let hub = model.hub, let sessionId else { return }
        do { _ = try await hub.mute(sessionId, muted: muted) } catch { self.error = error.localizedDescription }
        await refresh()
    }

    private func stop() async {
        guard let hub = model.hub, let sessionId else { return }
        do { _ = try await hub.stop(sessionId) } catch { self.error = error.localizedDescription }
        await refresh()
    }

    private func delete() async {
        guard let hub = model.hub, let sessionId else { return }
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

/// An icon over a word, in the session's action row.
struct ActionButton: View {
    let symbol: String
    let label: String
    var destructive = false
    let action: () -> Void
    @Environment(\.isEnabled) private var enabled

    var body: some View {
        Button(action: action) {
            VStack(spacing: 3) {
                Image(systemName: symbol).font(.system(size: 20, weight: .regular))
                Text(label).font(Face.geist(11, .medium, relativeTo: .caption2))
            }
            .foregroundStyle(destructive ? Ink.bad : Ink.sub)
            .frame(minWidth: 56, minHeight: 48)
            .opacity(enabled ? 1 : 0.4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

/// The pill at the bottom: a tap to type, the mic to speak, and on Home a plus to start.
struct AskBar: View {
    let placeholder: String
    var dark = false
    var start: (() -> Void)?
    let type: () -> Void
    let speak: () -> Void

    var body: some View {
        HStack(spacing: 4) {
            if let start {
                Button(action: start) {
                    Image(systemName: "plus").font(.system(size: 20, weight: .regular)).frame(width: 44, height: 44).contentShape(Circle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(Ink.ink)
                .accessibilityLabel("New session")
                .accessibilityIdentifier("startButton")
            }
            Button(action: type) {
                Text(placeholder)
                    .font(Face.geist(16, relativeTo: .body))
                    .foregroundStyle(Ink.mut)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                    .padding(.leading, start == nil ? 14 : 0)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier(start == nil ? "messageBar" : "typeButton")
            Button(action: speak) { MicDisc() }
                .buttonStyle(.plain)
                .accessibilityLabel(start == nil ? "Speak to this agent" : "Talk to your agents")
                .accessibilityIdentifier(start == nil ? "agentMic" : "micButton")
        }
        .padding(.leading, 6)
        .padding(.trailing, 8)
        .frame(height: 56)
        .background(Ink.sf, in: Capsule())
        .overlay(Capsule().strokeBorder(Ink.line))
        .shadow(color: .black.opacity(0.07), radius: 14, y: 10)
        .padding(.horizontal, 16)
        .padding(.top, 10)
        .padding(.bottom, 6)
        .background(dark ? Ink.term : Ink.bg)
    }
}
