import SwiftUI
import SwitchboardKit

/// An agent Switchboard didn't start, as the navigation stack opens it.
struct AgentRef: Hashable {
    let machine: String
    let name: String
}

enum Browse: Hashable {
    case allSessions, machines, projects
    case machine(String)
}

/// Home: what needs you, what's running, then lists. The mic is the main way in; the keyboard
/// is beside it.
struct HomeView: View {
    @Environment(AppModel.self) private var model
    @Environment(Voice.self) private var voice
    @State private var path = NavigationPath()
    @State private var starting = false
    @State private var showingDevices = false
    /// The last line each running session printed, by session id.
    @State private var lastLines: [String: String] = [:]

    private var waiting: [Schemas.Session] { model.sessions.filter { !$0.pendingApprovals.isEmpty } }
    private var running: [Schemas.Session] {
        model.sessions.filter { ["running", "starting"].contains($0.lifecycle) && $0.pendingApprovals.isEmpty }
    }
    private var finished: [Schemas.Session] {
        let since = Int64(Date().addingTimeInterval(-86_400).timeIntervalSince1970 * 1000)
        return model.sessions.filter { !["running", "starting"].contains($0.lifecycle) && $0.updatedAt > since }
    }

    var body: some View {
        @Bindable var model = model
        NavigationStack(path: $path) {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    summary
                    if let sent = model.lastSent, Date().timeIntervalSince(sent.at) < 600 { SentCard(sent: sent) { open($0) } }
                    ForEach(waiting, id: \.id) { s in
                        ForEach(s.pendingApprovals, id: \.id) { a in
                            ApprovalCard(approval: a, session: s, open: { path.append(s.id) }) {}
                        }
                    }
                    if !running.isEmpty { runningTiles }
                    if model.sessions.isEmpty && model.agents.isEmpty { empty }
                    lists
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 170)
            }
            .background(Color(.systemGroupedBackground))
            .accessibilityIdentifier("sessionsList")
            .navigationTitle("Agents")
            .navigationDestination(for: String.self) { SessionDetailView(sessionId: $0) }
            .navigationDestination(for: AgentRef.self) { AgentDetailView(agent: $0) }
            .navigationDestination(for: ConversationRef.self) { ConversationScreen(ref: $0) }
            .navigationDestination(for: Browse.self) { b in
                switch b {
                case .allSessions: AllSessionsView()
                case .machines: MachinesView()
                case .machine(let id): AllSessionsView(machine: id)
                case .projects: ProjectsView()
                }
            }
            .overlay(alignment: .bottom) { dock }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { starting = true } label: { Image(systemName: "plus") }
                        .accessibilityLabel("Start a session")
                        .accessibilityIdentifier("startButton")
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { showingDevices = true } label: { Image(systemName: "gearshape") }
                        .accessibilityLabel("Settings")
                        .accessibilityIdentifier("devicesButton")
                }
            }
            .refreshable { model.startPolling(); await model.refreshAgents() }
            .task(id: model.wakes) {
                while !Task.isCancelled {
                    await model.refreshAgents()
                    await loadLastLines()
                    try? await Task.sleep(for: .seconds(6))
                }
            }
        }
        .scaleEffect(voice.squeeze ? 0.982 : 1, anchor: .bottom)
        .animation(.spring(response: 0.3, dampingFraction: 1), value: voice.squeeze)
        .overlay { VoiceOverlay() }
        .sheet(isPresented: Binding(
            get: { voice.phase == .review },
            set: { if !$0, voice.phase == .review { voice.finish() } }
        )) { SendSheet() }
        .sheet(isPresented: $starting) {
            StartSheet { id in
                starting = false
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
    }

    private func open(_ t: Schemas.TargetView) {
        if let id = t.sessionId { path.append(id) } else if let name = t.agent { path.append(AgentRef(machine: t.machineId, name: name)) }
    }

    private var summary: some View {
        HStack(spacing: 7) {
            if model.connected {
                Circle().fill(.green).frame(width: 8, height: 8)
                let parts = [waiting.isEmpty ? nil : "\(waiting.count) need\(waiting.count == 1 ? "s" : "") you",
                             "\(running.count) running"].compactMap { $0 }
                Text(parts.joined(separator: " · "))
            } else {
                Image(systemName: "wifi.exclamationmark").foregroundStyle(.orange)
                Text("Can't reach the hub; retrying.")
            }
        }
        .font(.subheadline)
        .foregroundStyle(.secondary)
        .padding(.top, -8)
    }

    private var runningTiles: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Running").font(.title3.bold())
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 12) {
                    ForEach(running, id: \.id) { s in
                        NavigationLink(value: s.id) { RunningTile(session: s, lastLine: lastLines[s.id]) }
                            .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 16)
            }
            .padding(.horizontal, -16)
        }
    }

    private var empty: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Nothing running yet").font(.headline)
            Text("Tap the mic and say what to do, like “In kraft, fix the flaky retry test.”")
                .foregroundStyle(.secondary)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 20, style: .continuous))
    }

    private var lists: some View {
        VStack(alignment: .leading, spacing: 24) {
            if !finished.isEmpty {
                InsetGroup(title: "Finished today") {
                    ForEach(Array(finished.enumerated()), id: \.element.id) { i, s in
                        NavigationLink(value: s.id) {
                            ListRow(symbol: SessionStatus(s).symbol, tint: SessionStatus(s).color, title: s.label,
                                    subtitle: "\(SessionStatus(s).text) · \(s.projectId)", trailing: Ago.short(s.updatedAt), first: i == 0)
                        }
                    }
                }
            }
            if !model.agents.isEmpty {
                InsetGroup(title: "Started in Herdr") {
                    ForEach(Array(model.agents.enumerated()), id: \.element) { i, a in
                        NavigationLink(value: AgentRef(machine: a.machineId, name: a.name)) {
                            ListRow(symbol: "terminal", tint: .secondary, title: a.displayName,
                                    subtitle: "\(SessionStatus(agentStatus: a.agentStatus).text) · \(a.displayName == a.name ? a.machineId : a.herdrSession ?? a.machineId)",
                                    trailing: "", first: i == 0)
                        }
                    }
                }
            }
            InsetGroup(title: "Browse") {
                NavigationLink(value: Browse.allSessions) {
                    ListRow(symbol: "tray.full", tint: .secondary, title: "All sessions", subtitle: nil, trailing: "\(model.sessions.count + model.agents.count)", first: true)
                }
                NavigationLink(value: Browse.machines) {
                    ListRow(symbol: "desktopcomputer", tint: .accentColor, title: "Machines", subtitle: nil, trailing: "", first: false)
                }
                NavigationLink(value: Browse.projects) {
                    ListRow(symbol: "folder", tint: .accentColor, title: "Projects", subtitle: nil, trailing: "", first: false)
                }
            }
        }
        .buttonStyle(.plain)
    }

    private var dock: some View {
        VStack(spacing: 10) {
            Text("Tap and say what you want done").font(.footnote).foregroundStyle(.secondary)
            HStack {
                Button { voice.type(to: nil) } label: {
                    Image(systemName: "keyboard")
                        .font(.system(size: 20))
                        .frame(width: 50, height: 50)
                        .background(Color(.secondarySystemGroupedBackground), in: Circle())
                        .shadow(color: .black.opacity(0.08), radius: 6, y: 2)
                        .foregroundStyle(Color(.label))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Type instead")
                .accessibilityIdentifier("typeButton")
                Spacer()
                MicButton(size: 72) { voice.listen(to: nil) }
                    .accessibilityLabel("Talk to your agents")
                    .accessibilityIdentifier("micButton")
                Spacer()
                Color.clear.frame(width: 50, height: 50)
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 36)
        .padding(.bottom, 8)
        .background(alignment: .bottom) {
            LinearGradient(colors: [Color(.systemGroupedBackground).opacity(0), Color(.systemGroupedBackground)],
                           startPoint: .top, endPoint: UnitPoint(x: 0.5, y: 0.4))
                .ignoresSafeArea()
                .allowsHitTesting(false)
        }
    }

    /// Each tile's line comes from the conversation; reading the terminal would scroll it on the Mac.
    private func loadLastLines() async {
        guard let hub = model.hub else { return }
        for s in running.prefix(8) where s.lifecycle == "running" {
            guard let page = try? await hub.transcript(s.id) else { continue }
            let last = page.items.last { $0.kind != "user" && $0.text != nil }?.text
            lastLines[s.id] = last?.split(separator: "\n").first { !$0.trimmingCharacters(in: .whitespaces).isEmpty }.map(String.init)
        }
    }
}

/// The mic, which fades into the listening panel while voice mode is on.
struct MicButton: View {
    @Environment(Voice.self) private var voice
    let size: CGFloat
    let action: () -> Void

    var body: some View {
        let listening = voice.phase == .listening
        Button(action: action) {
            Image(systemName: "mic.fill")
                .font(.system(size: size * 0.38, weight: .semibold))
                .frame(width: size, height: size)
                .background(Color.accentColor, in: Circle())
                .foregroundStyle(.white)
                .shadow(color: Color.accentColor.opacity(0.35), radius: 12, y: 8)
        }
        .opacity(listening ? 0 : 1)
        .scaleEffect(listening ? 0.7 : 1)
        .animation(listening ? .easeOut(duration: 0.2) : .easeOut(duration: 0.25).delay(0.2), value: listening)
    }
}

private struct RunningTile: View {
    let session: Schemas.Session
    let lastLine: String?

    var body: some View {
        let status = SessionStatus(session)
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Circle().fill(status.color).frame(width: 7, height: 7)
                Text("\(status.text) · \(Ago.short(session.updatedAt))")
            }
            .font(.caption.weight(.semibold))
            .foregroundStyle(status.color)
            Text(session.label).font(.subheadline.weight(.semibold)).lineLimit(1)
            Text([session.projectId, session.machineId].joined(separator: " · "))
                .font(.footnote)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            Spacer(minLength: 0)
            Text(lastLine ?? " ")
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(.secondary)
                .lineLimit(2)
        }
        .padding(14)
        .frame(width: 168, height: 150, alignment: .topLeading)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 20, style: .continuous))
        .accessibilityElement(children: .combine)
    }
}

private struct SentCard: View {
    let sent: AppModel.Sent
    let open: (Schemas.TargetView) -> Void

    var body: some View {
        Button { if let t = sent.target { open(t) } } label: {
            VStack(alignment: .leading, spacing: 10) {
                Label(sent.started ? "Started \(sent.target?.label ?? "a session")" : "Sent to \(sent.target?.label ?? "the hub")",
                      systemImage: "checkmark")
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(Color.done)
                Text(sent.text)
                    .padding(.horizontal, 13)
                    .padding(.vertical, 9)
                    .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                    .foregroundStyle(.white)
                    .multilineTextAlignment(.leading)
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 20, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("sentCard")
    }
}

/// A titled inset group, like a grouped list's section.
struct InsetGroup<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.footnote)
                .foregroundStyle(.secondary)
                .textCase(.uppercase)
                .padding(.horizontal, 16)
            VStack(spacing: 0) { content }
                .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        }
    }
}

struct ListRow: View {
    let symbol: String
    let tint: Color
    let title: String
    let subtitle: String?
    let trailing: String
    let first: Bool

    var body: some View {
        HStack(spacing: 14) {
            Image(systemName: symbol)
                .font(.system(size: 19))
                .foregroundStyle(tint)
                .frame(width: 26)
            HStack(spacing: 8) {
                VStack(alignment: .leading, spacing: 1) {
                    Text(title).foregroundStyle(.primary).lineLimit(1)
                    if let subtitle { Text(subtitle).font(.subheadline).foregroundStyle(.secondary).lineLimit(1) }
                }
                Spacer(minLength: 0)
                Text(trailing).foregroundStyle(.secondary).monospacedDigit()
                Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary)
            }
            .padding(.vertical, 11)
            .padding(.trailing, 16)
            .overlay(alignment: .top) { if !first { Divider() } }
        }
        .padding(.leading, 16)
        .frame(minHeight: 44)
        .contentShape(Rectangle())
    }
}
