import SwiftUI
import SwitchboardKit

enum HomeTab: Hashable { case now, spaces, projects }

/// Home: the hub's state, a sentence saying what needs you, then Now, Spaces and Projects. The
/// ask bar stays at the bottom: plus to start a session, a tap to type, the mic to speak.
struct HomeView: View {
    @Environment(AppModel.self) private var model
    @Environment(Voice.self) private var voice
    @State private var path = NavigationPath()
    @State private var tab = HomeTab.now
    /// The project New session opens with; set by a Projects row.
    @State private var starting: StartRequestDraft?
    @State private var showingDevices = false
    /// The last line each busy agent said, by its id.
    @State private var lastLines: [String: String] = [:]
    @State private var pacer = FallbackPacer()
    /// Connecting has taken long enough to say so.
    @State private var slow = false

    private var spaces: [AgentSpace] { model.spaces(since: HerdrSession.startOfToday()) }
    private var approvals: [(session: Schemas.Session, approval: Schemas.Approval)] {
        model.sessions.flatMap { s in s.pendingApprovals.map { (s, $0) } }.sorted { $0.approval.deadline < $1.approval.deadline }
    }
    /// Live agents, each with the space it's in.
    private var live: [Item] {
        spaces.flatMap { sp in sp.live.map { Item(agent: $0, space: sp) } }
    }
    /// Waiting on you at a dialog or stuck, apart from approvals, which have their own cards.
    private var waiting: [Item] {
        live.filter { i in
            guard i.agent.needsYou else { return false }
            if case .task(let s) = i.agent { return s.pendingApprovals.isEmpty }
            return true
        }
    }
    private var working: [Item] { live.filter { $0.agent.agentStatus == "working" } }
    private var finished: [Item] {
        let done = live.filter { $0.agent.agentStatus == "done" }
        let since = HerdrSession.startOfToday()
        let ended = spaces.flatMap { sp in
            sp.ended.compactMap { a -> Item? in
                if case .task(let s) = a, s.updatedAt >= since { return Item(agent: a, space: sp) }
                return nil
            }
        }
        return done + ended
    }

    var body: some View {
        NavigationStack(path: $path) {
            VStack(spacing: 0) {
                top
                headline
                    .stateStyle()
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 20)
                    .padding(.top, 8)
                    .accessibilityIdentifier("headline")
                if !model.loaded && model.unreachable {
                    HubUnreachable(state: ConnectState(model, slow: slow))
                } else {
                    TextTabs(tabs: [(.now, "Now"), (.spaces, "Spaces"), (.projects, "Projects")], selection: $tab.animation(.snappy))
                        .padding(.top, 14)
                    TabView(selection: $tab) {
                        page { now }.tag(HomeTab.now)
                        page { SpacesList() }.tag(HomeTab.spaces)
                        page { ProjectsList { starting = StartRequestDraft(project: $0) } }.tag(HomeTab.projects)
                    }
                    .tabViewStyle(.page(indexDisplayMode: .never))
                }
                AskBar(placeholder: "Say or type what you want done",
                       start: { starting = StartRequestDraft(project: nil) },
                       type: { voice.type(to: nil) }, speak: { voice.listen(to: nil) })
            }
            .inboxPage()
            .navigationDestination(for: String.self) { AgentPage(source: .session($0)) }
            .navigationDestination(for: AgentRef.self) { AgentPage(source: .agent(machine: $0.machine, name: $0.name)) }
            .navigationDestination(for: AgentPageRef.self) { AgentPage(source: $0.source, tab: $0.tab) }
            .navigationDestination(for: DecisionRef.self) { DecisionPage(ref: $0) }
            .navigationDestination(for: DiffRef.self) { DiffPage(ref: $0) }
            .navigationDestination(for: AgentSpace.Key.self) { SpacePage(key: $0) }
            .navigationDestination(for: Browse.self) { b in
                switch b {
                case .machines: MachinesPage()
                case .machine(let id): MachineSpacesPage(machine: id)
                }
            }
            .task(id: model.loaded) {
                slow = false
                guard !model.loaded else { return }
                try? await Task.sleep(for: .seconds(8))
                if !Task.isCancelled { slow = true }
            }
            .task(id: model.followKey) {
                while model.followKey != nil, !Task.isCancelled {
                    await model.refreshAgents(activity: true)
                    await loadLastLines()
                    try? await Task.sleep(for: .seconds(5))
                }
            }
        }
        .environment(\.goHome, path.count > 1 ? { path = NavigationPath() } : nil)
        .environment(\.replaceTop) { renamed in
            var quiet = Transaction()
            quiet.disablesAnimations = true
            withTransaction(quiet) {
                if !path.isEmpty { path.removeLast() }
                switch renamed {
                case .agent(let ref): path.append(ref)
                case .space(let key): path.append(key)
                }
            }
        }
        .scaleEffect(voice.squeeze ? 0.982 : 1, anchor: .bottom)
        .animation(.spring(response: 0.3, dampingFraction: 1), value: voice.squeeze)
        .overlay(alignment: .top) {
            if let banner = model.banner {
                Button { model.banner = nil } label: { Banner(text: banner) }
                    .buttonStyle(.plain)
                    .padding(.horizontal, 16)
                    .padding(.top, 4)
                    .shadow(color: .black.opacity(0.08), radius: 10, y: 4)
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .accessibilityIdentifier("banner")
                    .task(id: banner) {
                        try? await Task.sleep(for: .seconds(6))
                        if model.banner == banner { model.banner = nil }
                    }
            }
        }
        .animation(.snappy, value: model.banner)
        .overlay { VoiceOverlay() }
        .sheet(isPresented: Binding(
            get: { voice.phase == .review && !voice.dictating },
            set: { if !$0, voice.phase == .review, !voice.dictating { voice.finish() } }
        )) { SendSheet() }
        .sheet(item: $starting) { draft in
            StartSheet(project: draft.project) { id in
                starting = nil
                if let id { path.append(id) }
            }
        }
        .sheet(isPresented: $showingDevices) { DevicesView() }
        // `initial`: a tap that launched the app set this before the list appeared.
        .onChange(of: model.openSessionId, initial: true) { _, id in
            if let id {
                path = NavigationPath([id])
                model.openSessionId = nil
            }
        }
        // Opens the agent that asked, and says so when its question is no longer open. The id is
        // cleared only after the lookup: clearing it first would cancel this very task.
        .task(id: model.openAttentionId) {
            guard let id = model.openAttentionId, let hub = model.hub else { return }
            let a: Schemas.Attention
            do {
                a = try await hub.attention(id)
            } catch {
                if Task.isCancelled { return }
                if model.openAttentionId == id { model.openAttentionId = nil }
                model.banner = "Couldn't reach the hub to open that notification. Try again in a moment."
                return
            }
            if model.openAttentionId == id { model.openAttentionId = nil }
            // Its name may belong to another agent by now; that one isn't opened in its place.
            if a.sessionId == nil, a.sameAgent == false {
                model.banner = "The agent that asked has ended, so there's nothing to open."
                return
            }
            switch a.state {
            case "open": break
            case "answered": model.banner = "That question was already answered."
            case "expired": model.banner = "That question expired. Here's the agent now."
            default: model.banner = "The agent has moved on from that question. Here it is now."
            }
            if let s = a.sessionId { path = NavigationPath([s]) } else { path = NavigationPath([AgentRef(machine: a.machineId, name: a.agent)]) }
        }
    }

    private var top: some View {
        HStack(spacing: 8) {
            HStack(spacing: 8) {
                if model.connected {
                    Circle().fill(Ink.ok).frame(width: 8, height: 8)
                    Text(hubLine)
                } else if model.unreachable {
                    Circle().fill(Ink.warn).frame(width: 8, height: 8)
                    Text("Can’t reach the hub · retrying")
                } else {
                    Circle().fill(Ink.mut).frame(width: 8, height: 8)
                    Text("Connecting…")
                }
            }
            .font(Face.geist(13, .medium, relativeTo: .footnote))
            .foregroundStyle(Ink.sub)
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("hubState")
            Spacer()
            Button { showingDevices = true } label: {
                Image(systemName: "slider.horizontal.3")
                    .font(.system(size: 19, weight: .regular))
                    .frame(width: 44, height: 44)
                    .contentShape(Circle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(Ink.ink)
            .accessibilityLabel("Settings")
            .accessibilityIdentifier("devicesButton")
        }
        .padding(.leading, 20)
        .padding(.trailing, 10)
        .frame(height: 44)
    }

    /// One tab's list. Until the hub's first answer, placeholders: empty lists would read as "nothing running".
    private func page<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                if model.loaded { content() } else { LoadingRows() }
            }
            .padding(.horizontal, 20)
            .padding(.top, 14)
            .padding(.bottom, 24)
        }
        .refreshable { model.startPolling(); await model.refreshAgents(activity: true) }
    }

    private var hubLine: String {
        let m = model.machines
        if m.count == 1 { return "\(m[0].id) · connected" }
        if let d = m.first(where: \._default), m.count > 1 { return "\(d.id) and \(m.count - 1) more · connected" }
        return "Connected"
    }

    private var headline: Headline {
        if !model.loaded { return ConnectState(model, slow: slow).headline }
        switch tab {
        case .now:
            let needs = approvals.count + waiting.count
            let w = working.count
            if needs == 0 && live.isEmpty && finished.isEmpty { return Headline("Nothing running yet.", "Say what you want done.") }
            var first = needs == 0 ? "Nothing needs you." : "\(needs) \(needs == 1 ? "thing needs" : "things need") you."
            if model.away != nil { first = "Welcome back. " + first }
            let second = w == 0 ? "Nothing working." : "\(w) \(w == 1 ? "agent" : "agents") working."
            return Headline(first, second)
        case .spaces:
            let all = model.spaces(since: nil)
            let liveCount = all.filter { !$0.isEnded }.count
            let need = all.filter { $0.glyph == .need }.count
            if liveCount == 0 { return Headline("No spaces live.", all.isEmpty ? nil : "\(all.count) ended.") }
            return Headline("\(liveCount) \(liveCount == 1 ? "space" : "spaces") live.", need == 0 ? "None need you." : "\(need) need\(need == 1 ? "s" : "") you.")
        case .projects:
            let n = model.projects.count
            return Headline("\(n) \(n == 1 ? "project" : "projects").", "Tap one to start.")
        }
    }

    // Now.

    @ViewBuilder
    private var now: some View {
        if let away = model.away {
            AwayCard(activity: away).padding(.bottom, 14)
        }
        if let sent = model.lastSent, Date().timeIntervalSince(sent.at) < 600 {
            SentRow(sent: sent) { open($0) }
        }
        if let first = approvals.first {
            ApprovalCard(approval: first.approval, session: first.session, open: { path.append(first.session.id) }) {}
                .padding(.bottom, 6)
            ForEach(Array(approvals.dropFirst().enumerated()), id: \.element.approval.id) { i, a in
                NavigationLink(value: a.session.id) {
                    Row(title: Self.approvalTitle(a.approval, a.session), subtitle: a.approval.inputText, subtitleMono: a.approval.kind == "permission",
                        first: i == 0) {
                        StatusGlyph(glyph: .need)
                    } trailing: {
                        WhereTag(backend: a.session.backend ?? "herdr")
                    }
                }
                .buttonStyle(.plain)
            }
        }
        ForEach(Array(waiting.enumerated()), id: \.element.id) { i, item in
            needRow(item, first: i == 0 && approvals.count <= 1)
        }
        if !working.isEmpty {
            SectionHead("Working", count: working.count)
            ForEach(Array(working.enumerated()), id: \.element.id) { i, item in
                AgentLink(agent: item.agent) {
                    Row(title: item.label, subtitle: lastLines[item.id] ?? item.agent.title.flatMap { $0 == item.agent.nameInSpace ? nil : $0 } ?? "Working",
                        subtitleMono: lastLines[item.id] != nil,
                        first: i == 0) {
                        StatusGlyph(glyph: .work)
                    } trailing: {
                        WhereTag(backend: item.space.backend)
                    }
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("busy-\(item.label)")
            }
        }
        if !finished.isEmpty {
            SectionHead("Finished today", count: finished.count)
            ForEach(Array(finished.enumerated()), id: \.element.id) { i, item in
                AgentLink(agent: item.agent) {
                    Row(title: item.label, subtitle: lastLines[item.id] ?? item.agent.title ?? item.agent.subtitle, first: i == 0) {
                        StatusGlyph(glyph: item.agent.isEnded ? .end : .done)
                    } trailing: {
                        if case .task(let s) = item.agent, item.agent.isEnded {
                            Text(Ago.short(s.updatedAt)).font(Face.meta).foregroundStyle(Ink.mut)
                        } else {
                            WhereTag(backend: item.space.backend)
                        }
                    }
                }
                .buttonStyle(.plain)
            }
        }
        if live.isEmpty && finished.isEmpty && approvals.isEmpty {
            empty
        } else if approvals.isEmpty && waiting.isEmpty && working.isEmpty && finished.isEmpty {
            StateRow(text: "\(live.count) \(live.count == 1 ? "agent is" : "agents are") idle. They're under Spaces.")
        }
    }

    private static func approvalTitle(_ a: Schemas.Approval, _ s: Schemas.Session) -> String {
        switch a.kind {
        case "question": "\(s.label) has a question"
        case "plan": "\(s.label) has a plan for you"
        default: "\(s.label) wants to run"
        }
    }

    @ViewBuilder
    private func needRow(_ item: Item, first: Bool) -> some View {
        let subtitle: String = switch item.agent {
        case .task(let s) where s.hookFailed: "Stuck at a dialog · answer it at its terminal"
        default: item.agent.title.flatMap { $0 == item.agent.nameInSpace ? nil : "Waiting for you · \($0)" } ?? "Waiting for you"
        }
        let row = Row(title: item.label, subtitle: subtitle, subtitleColor: Ink.warn, first: first) {
            StatusGlyph(glyph: .need)
        } trailing: {
            WhereTag(backend: item.space.backend)
        }
        Group {
            switch item.agent {
            case .live(let a):
                NavigationLink(value: DecisionRef(machine: a.machineId, agent: a.name, label: item.label,
                                                  meta: [a.kind.map { $0 == "claude" ? "Claude Code" : $0.capitalized }, a.machineId].compactMap { $0 }.joined(separator: " · "))) { row }
            case .task(let s):
                NavigationLink(value: s.id) { row }
            case .ended:
                row
            }
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("needs-\(item.label)")
    }

    @ViewBuilder
    private var empty: some View {
        SectionHead("Try saying")
        Button { voice.listen(to: nil) } label: {
            if let p = model.projects.first?.id {
                example("“In \(p), fix the flaky retry test.”", "Starts a new task in \(p), on its own branch", first: true)
            } else {
                example("“Fix the flaky retry test.”", "Starts a new task, on its own branch", first: true)
            }
        }
        .buttonStyle(.plain)
        Button { voice.listen(to: nil) } label: {
            example("“What is everyone working on?”", "Switchboard picks where it goes, and asks if unsure", first: false)
        }
        .buttonStyle(.plain)
        SectionHead("Ready on this hub")
        Button { tab = .projects } label: {
            let p = model.projects.map(\.id)
            Row(title: "\(p.count) \(p.count == 1 ? "project" : "projects")",
                subtitle: p.count > 3 ? "\(p.prefix(3).joined(separator: ", ")) and \(p.count - 3) more" : p.joined(separator: ", "),
                first: true, leading: { EmptyView() }, trailing: { Chevron() })
        }
        .buttonStyle(.plain)
        NavigationLink(value: Browse.machines) {
            let m = model.machines
            let ready = m.filter { $0.status == "ready" }.map(\.id)
            Row(title: "\(m.count) \(m.count == 1 ? "machine" : "machines")",
                subtitle: ready.isEmpty ? "None ready" : "\(ready.joined(separator: " and ")) \(ready.count == 1 ? "is" : "are") ready",
                leading: { EmptyView() }, trailing: { Chevron() })
        }
        .buttonStyle(.plain)
        Hairline()
    }

    private func example(_ said: String, _ what: String, first: Bool) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(said).font(Face.geist(20, .medium)).tracking(-0.4).foregroundStyle(Ink.ink).fixedSize(horizontal: false, vertical: true)
            Text(what).font(Face.sub).foregroundStyle(Ink.sub)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 12)
        .overlay(alignment: .top) { if !first { Hairline() } }
        .contentShape(Rectangle())
    }

    private func open(_ t: Schemas.TargetView) {
        if let id = t.sessionId { path.append(id) } else if let name = t.agent { path.append(AgentRef(machine: t.machineId, name: name)) }
    }

    /// Each row's line comes from the hub's activity list; where that doesn't cover its machine,
    /// from the conversation at most every 30 s. Reading the terminal would scroll it on the Mac.
    private func loadLastLines() async {
        guard let hub = model.hub else { return }
        var reads = 0
        for t in working + finished.filter({ !$0.agent.isEnded }) {
            switch model.tileLines?.line(for: t.agent) ?? .unknown {
            case .known(let line):
                lastLines[t.id] = line
            case .unknown:
                guard reads < 8, pacer.due(t.id) else { continue }
                reads += 1
                let page: Schemas.Transcript?
                switch t.agent {
                case .task(let s): page = try? await hub.transcript(s.id)
                case .live(let a): page = try? await hub.agentTranscript(machine: a.machineId, agent: a.name)
                case .ended: page = nil
                }
                guard let page else { continue }
                let last = page.items.last { $0.kind != "user" && $0.text != nil }?.text
                lastLines[t.id] = last?.split(separator: "\n").first { !$0.trimmingCharacters(in: .whitespaces).isEmpty }.map(String.init)
            }
        }
    }
}

/// What New session opens with.
struct StartRequestDraft: Identifiable {
    let id = UUID()
    let project: String?
}

/// A live agent and the space it's in.
private struct Item: Identifiable {
    let agent: HerdrSession.Agent
    let space: AgentSpace
    var label: String { space.label(agent) }
    var id: String {
        switch agent {
        case .task(let s): s.id
        case .live(let a): "\(a.machineId)/\(a.name)"
        case .ended(let e): "\(e.machineId)/\(e.name)/\(e.endedAt)"
        }
    }
}

/// The last thing sent or started from Home, shown for ten minutes.
private struct SentRow: View {
    let sent: AppModel.Sent
    let open: (Schemas.TargetView) -> Void

    var body: some View {
        Button { if let t = sent.target { open(t) } } label: {
            Row(title: sent.started ? "Started \(sent.target?.label ?? "a session")" : "Sent to \(sent.target?.label ?? "the hub")",
                subtitle: "“\(sent.text)”", first: true) {
                StatusGlyph(glyph: .done)
            } trailing: {
                Text(Ago.short(Int64(sent.at.timeIntervalSince1970 * 1000))).font(Face.meta).foregroundStyle(Ink.mut)
            }
        }
        .buttonStyle(.plain)
        .padding(.bottom, 6)
        .accessibilityIdentifier("sentCard")
    }
}
