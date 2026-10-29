import SwiftUI
import SwitchboardKit

/// Whose conversation: a Switchboard session's, or an agent started in Herdr.
enum ConversationSource: Hashable {
    case session(String)
    case agent(machine: String, name: String)
}

/// What the navigation stack opens to show a whole conversation.
struct ConversationRef: Hashable {
    let source: ConversationSource
    let title: String
    /// Opens on the agent's screen instead of the conversation.
    var screen = false
}

/// One agent's conversation, and its terminal, followed while a screen shows them.
@MainActor
@Observable
final class ConversationModel {
    let source: ConversationSource
    private(set) var conversation = Conversation()
    /// Why there's no conversation, e.g. no log yet or an older hub; the terminal stands in.
    private(set) var unavailable: String?
    private(set) var terminal = ""
    /// The same screen with its colours, when the hub sends them.
    private(set) var terminalAnsi: String?
    /// False until the screen has been read once.
    private(set) var terminalLoaded = false
    private(set) var terminalError: String?
    private(set) var error: String?
    private(set) var loadingEarlier = false
    /// The terminal is read only while it's on screen or standing in.
    var wantsTerminal = false

    init(_ source: ConversationSource) {
        self.source = source
    }

    /// Polls until cancelled.
    func follow(_ model: AppModel) async {
        while !Task.isCancelled {
            if let hub = model.hub {
                await refresh(hub)
                if wantsTerminal || unavailable != nil { await loadTerminal(hub) }
            }
            try? await Task.sleep(for: .seconds(3))
        }
    }

    func refresh(_ hub: Hub) async {
        do {
            if conversation.isLoaded {
                let after = conversation.end
                conversation.append(try await page(hub, after: after, before: nil), askedAfter: after)
            } else {
                conversation.replace(with: try await page(hub, after: nil, before: nil))
            }
            unavailable = nil
            error = nil
        } catch let e as HubError where e.code == "http_404" {
            // A hub older than this app has no conversations to give.
            unavailable = "the hub needs updating"
        } catch let e as HubError where ["bad_request", "not_found"].contains(e.code ?? "") {
            unavailable = e.localizedDescription
        } catch {
            if !Task.isCancelled { self.error = error.localizedDescription }
        }
    }

    func loadEarlier(_ hub: Hub) async {
        guard let start = conversation.start, start > 0, !loadingEarlier else { return }
        loadingEarlier = true
        defer { loadingEarlier = false }
        do {
            conversation.prepend(try await page(hub, after: nil, before: start))
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// Only the screen: reading further back would scroll the agent's terminal on the Mac.
    func loadTerminal(_ hub: Hub) async {
        do {
            let out = switch source {
            case .session(let id): try await hub.output(id, visible: true, ansi: true)
            case .agent(let machine, let name): try await hub.agentOutput(machine: machine, agent: name, visible: true, ansi: true)
            }
            terminal = out.text
            terminalAnsi = out.ansi
            terminalLoaded = true
            terminalError = nil
        } catch {
            if !Task.isCancelled { terminalError = error.localizedDescription }
        }
    }

    private func page(_ hub: Hub, after: Int64?, before: Int64?) async throws -> Schemas.Transcript {
        switch source {
        case .session(let id): try await hub.transcript(id, after: after, before: before)
        case .agent(let machine, let name): try await hub.agentTranscript(machine: machine, agent: name, after: after, before: before)
        }
    }
}

/// The whole conversation, with the terminal a tab away.
struct ConversationScreen: View {
    @Environment(AppModel.self) private var model
    @Environment(Voice.self) private var voice
    let ref: ConversationRef
    @State private var convo: ConversationModel
    @State private var tab: Tab
    /// Whether the conversation is scrolled to its latest message.
    @State private var atEnd = true

    enum Tab: String, CaseIterable {
        case conversation = "Conversation"
        case terminal = "Screen"
    }

    init(ref: ConversationRef) {
        self.ref = ref
        _convo = State(initialValue: ConversationModel(ref.source))
        _tab = State(initialValue: ref.screen ? .terminal : .conversation)
    }

    /// Where the message bar sends.
    private var target: Schemas.TargetView? {
        switch ref.source {
        case .session(let id): model.sessions.first { $0.id == id }.map(Schemas.TargetView.session)
        case .agent(let machine, let name): .agent(machine: machine, name: name, label: ref.title)
        }
    }

    /// Who replies: `Claude` or `Codex`.
    private var speaker: String {
        switch ref.source {
        case .session(let id): model.sessions.first { $0.id == id }?.agent.capitalized ?? "Agent"
        case .agent(let machine, let name):
            model.agents.first { $0.machineId == machine && $0.name == name }?.kind?.capitalized ?? "Agent"
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            if let why = convo.unavailable {
                // Nothing to switch between: the terminal is all there is.
                Label("No conversation to show (\(why)), so here's the agent's screen.", systemImage: "info.circle")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                terminal
            } else {
                Picker("View", selection: $tab) {
                    ForEach(Tab.allCases, id: \.self) { Text($0.rawValue) }
                }
                .pickerStyle(.segmented)
                .padding(.horizontal, 16)
                .padding(.vertical, 8)
                if tab == .terminal { terminal } else { feed }
            }
        }
        .safeAreaInset(edge: .bottom) { messageBar }
        .background(Color(.systemGroupedBackground))
        .navigationTitle(ref.title)
        .navigationBarTitleDisplayMode(.inline)
        .onChange(of: tab, initial: true) { _, t in
            convo.wantsTerminal = t == .terminal
            // Shown at once, not at the next refresh.
            if t == .terminal, let hub = model.hub { Task { await convo.loadTerminal(hub) } }
        }
        .task(id: model.wakes) { await convo.follow(model) }
    }

    @ViewBuilder
    private var terminal: some View {
        if convo.terminalLoaded {
            TerminalView(text: convo.terminal, ansi: convo.terminalAnsi)
        } else if let error = convo.terminalError {
            Waiting(symbol: "exclamationmark.triangle", text: "Couldn't read the screen: \(error)", busy: false)
        } else {
            Waiting(symbol: "terminal", text: "Loading the screen…", busy: true)
        }
    }

    private var feed: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    if convo.conversation.hasEarlier {
                        Button {
                            Task { if let hub = model.hub { await convo.loadEarlier(hub) } }
                        } label: {
                            if convo.loadingEarlier { ProgressView() } else { Text("Show earlier messages") }
                        }
                        .frame(maxWidth: .infinity)
                    }
                    if !convo.conversation.isLoaded {
                        if let error = convo.error {
                            Waiting(symbol: "exclamationmark.triangle", text: "Couldn't load the conversation: \(error)", busy: false)
                                .padding(.top, 40)
                        } else {
                            ConversationSkeleton()
                        }
                    } else if convo.conversation.items.isEmpty {
                        Waiting(symbol: "bubble.left.and.bubble.right", text: "Nothing said yet.", busy: false).padding(.top, 40)
                    }
                    ForEach(Turns.entries(convo.conversation.items)) { EntryRow(entry: $0, speaker: speaker) }
                    if convo.conversation.isLoaded {
                        if let status { StatusLine(status: status) }
                        if let error = convo.error { Text(error).font(.footnote).foregroundStyle(.orange) }
                    }
                    Color.clear.frame(height: 1).id(Self.end)
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 24)
            }
            .defaultScrollAnchor(.bottom)
            .modifier(TracksEnd(atEnd: $atEnd))
            // The first page replaces the loading placeholder, whose offset leaves the lazy stack showing
            // nothing until scrolled; new messages follow only if you were at the end.
            .onChange(of: convo.conversation.items.last?.id, initial: true) { old, _ in
                if old == nil || atEnd { proxy.scrollTo(Self.end, anchor: .bottom) }
            }
            .overlay(alignment: .bottom) {
                if !atEnd {
                    Button {
                        withAnimation(.snappy) { proxy.scrollTo(Self.end, anchor: .bottom) }
                    } label: {
                        Image(systemName: "arrow.down")
                            .font(.body.weight(.semibold))
                            .frame(width: 40, height: 40)
                            .background(Color(.secondarySystemGroupedBackground), in: Circle())
                            .overlay(Circle().stroke(Color(.separator).opacity(0.6), lineWidth: 0.5))
                            .shadow(color: .black.opacity(0.15), radius: 6, y: 2)
                            .frame(width: 44, height: 44)
                            .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(.primary)
                    .padding(.bottom, 10)
                    .transition(.opacity.combined(with: .scale(scale: 0.8)))
                    .accessibilityLabel("Jump to the latest")
                    .accessibilityIdentifier("jumpToLatest")
                }
            }
            .animation(.easeOut(duration: 0.15), value: atEnd)
            .accessibilityIdentifier("conversation")
        }
    }

    private static let end = "end"

    /// Opens the review sheet, where `/` lists the agent's skills.
    private var messageBar: some View {
        HStack(spacing: 8) {
            Button { voice.type(to: target) } label: {
                Text("Message, or / for skills")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 40, alignment: .leading)
                    .padding(.horizontal, 16)
                    .background(Color(.secondarySystemGroupedBackground), in: Capsule())
                    .overlay(Capsule().stroke(Color(.separator), lineWidth: 0.5))
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("messageBar")
            Button { voice.listen(to: target) } label: {
                Image(systemName: "mic.fill")
                    .font(.system(size: 18, weight: .semibold))
                    .frame(width: 44, height: 44)
                    .background(Color.accentColor, in: Circle())
                    .foregroundStyle(.white)
            }
            .accessibilityLabel("Talk to \(ref.title)")
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 6)
        .background(.bar)
        .disabled(target == nil)
    }

    private var status: SessionStatus? {
        switch ref.source {
        case .session(let id): model.sessions.first { $0.id == id }.map(SessionStatus.init)
        case .agent(let machine, let name):
            model.agents.first { $0.machineId == machine && $0.name == name }.map { SessionStatus(agentStatus: $0.agentStatus) }
        }
    }
}

/// A Markdown table as a grid: each column's width in proportion to its longest cell, long cells
/// wrapping. More than four columns scroll sideways.
struct TableView: View {
    let table: MarkdownTable

    var body: some View {
        if table.columns > 4 {
            ScrollView(.horizontal, showsIndicators: false) {
                grid.frame(width: CGFloat(table.weights.reduce(0, +)) * 8 + 24)
            }
        } else {
            grid
        }
    }

    private var grid: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !table.header.isEmpty {
                WeightedRow(weights: table.weights) {
                    ForEach(Array(table.header.enumerated()), id: \.offset) { _, cell in
                        Text(MarkdownText.inline(cell)).font(.footnote.weight(.semibold)).foregroundStyle(.secondary)
                    }
                }
                .padding(.vertical, 6)
            }
            ForEach(Array(table.rows.enumerated()), id: \.offset) { i, row in
                if i > 0 || !table.header.isEmpty { Divider() }
                WeightedRow(weights: table.weights) {
                    ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                        Text(MarkdownText.inline(cell)).font(.subheadline)
                    }
                }
                .padding(.vertical, 6)
            }
        }
    }
}

/// Cells side by side, each given its weight's share of the width and as tall as it needs.
struct WeightedRow: Layout {
    let weights: [Double]
    var spacing: CGFloat = 10

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width ?? 320
        let heights = zip(subviews, widths(width, count: subviews.count)).map {
            $0.sizeThatFits(ProposedViewSize(width: $1, height: nil)).height
        }
        return CGSize(width: width, height: heights.max() ?? 0)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX
        for (view, width) in zip(subviews, widths(bounds.width, count: subviews.count)) {
            view.place(at: CGPoint(x: x, y: bounds.minY), proposal: ProposedViewSize(width: width, height: nil))
            x += width + spacing
        }
    }

    private func widths(_ total: CGFloat, count: Int) -> [CGFloat] {
        let w = Array(weights.prefix(count))
        let sum = max(w.reduce(0, +), 1)
        let available = max(total - spacing * CGFloat(max(count - 1, 0)), 0)
        return w.map { available * CGFloat($0 / sum) }
    }
}

/// What the agent last said and what its work since your last message came to, for a session
/// or agent page.
struct SummaryCard: View {
    let convo: ConversationModel
    /// Where "Read all" goes.
    let conversation: ConversationRef

    var body: some View {
        let latest = Turns.latest(convo.conversation.items)
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Latest reply").font(.caption.weight(.semibold)).foregroundStyle(.secondary).textCase(.uppercase)
                Spacer()
                if let at = latest.at.flatMap(Turns.time) { Text(at).font(.caption).foregroundStyle(.secondary) }
            }
            if let why = convo.unavailable {
                Label("No conversation to show (\(why)).", systemImage: "info.circle")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            } else if !convo.conversation.isLoaded {
                ConversationSkeleton(rows: 1)
            } else if let reply = latest.reply?.text {
                Clamped(height: 286) { MarkdownText(text: reply) }
            } else {
                Text("Nothing said yet.").font(.subheadline).foregroundStyle(.secondary)
            }
            HStack {
                if !latest.tags.isEmpty { TagRow(tags: latest.tags) }
                Spacer(minLength: 12)
                if latest.reply != nil {
                    NavigationLink(value: conversation) {
                        Text("Read all").font(.subheadline.weight(.semibold)).frame(minHeight: 44)
                    }
                    .accessibilityIdentifier("readAll")
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.top, 14)
        .padding(.bottom, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .accessibilityIdentifier("latest")
    }
}

/// At most `height` tall; cut-off content fades out.
private struct Clamped<Content: View>: View {
    let height: CGFloat
    @ViewBuilder let content: Content
    @State private var full: CGFloat = 0

    var body: some View {
        content
            .fixedSize(horizontal: false, vertical: true)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { full = $0 }
            .frame(maxHeight: height, alignment: .top)
            .clipped()
            .mask {
                VStack(spacing: 0) {
                    Rectangle()
                    if full > height { LinearGradient(colors: [.black, .clear], startPoint: .top, endPoint: .bottom).frame(height: 56) }
                }
            }
    }
}

/// Keeps `atEnd` up to date where iOS can say where the scroll view is; elsewhere it stays true.
private struct TracksEnd: ViewModifier {
    @Binding var atEnd: Bool

    func body(content: Content) -> some View {
        if #available(iOS 18.0, *) {
            content.onScrollGeometryChange(for: Bool.self) { g in
                g.contentOffset.y + g.containerSize.height >= g.contentSize.height - 80
            } action: { _, now in
                atEnd = now
            }
        } else {
            content
        }
    }
}

/// The two ways into an agent: its conversation and its screen.
struct ConversationLinks: View {
    let source: ConversationSource
    let title: String
    let hasConversation: Bool

    var body: some View {
        VStack(spacing: 0) {
            if hasConversation {
                NavigationLink(value: ConversationRef(source: source, title: title)) {
                    ListRow(symbol: "bubble.left.and.bubble.right", tint: .accentColor, title: "Conversation",
                            subtitle: nil, trailing: "", first: true)
                }
                .accessibilityIdentifier("openConversation")
            }
            NavigationLink(value: ConversationRef(source: source, title: title, screen: true)) {
                ListRow(symbol: "terminal", tint: .accentColor, title: "Screen",
                        subtitle: nil, trailing: "", first: !hasConversation)
            }
            .accessibilityIdentifier("openScreen")
        }
        .buttonStyle(.plain)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
    }
}

/// A message, a reply and a tool call drawn as grey shapes that pulse while the conversation loads.
struct ConversationSkeleton: View {
    var rows = 3
    @State private var dim = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(0..<rows, id: \.self) { _ in
                RoundedRectangle(cornerRadius: 18).frame(width: 190, height: 38)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                VStack(alignment: .leading, spacing: 7) {
                    Capsule().frame(height: 12)
                    Capsule().frame(height: 12).padding(.trailing, 40)
                    Capsule().frame(width: 150, height: 12)
                }
                RoundedRectangle(cornerRadius: 12).frame(height: 38)
            }
        }
        .foregroundStyle(Color(.systemFill))
        .opacity(dim ? 0.45 : 1)
        .animation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true), value: dim)
        .onAppear { dim = true }
        .accessibilityElement()
        .accessibilityLabel("Loading the conversation")
    }
}

/// A centred symbol and line, with a spinner while something's on its way.
struct Waiting: View {
    let symbol: String
    let text: String
    let busy: Bool

    var body: some View {
        VStack(spacing: 12) {
            if busy { ProgressView() } else { Image(systemName: symbol).font(.title2).foregroundStyle(.secondary) }
            Text(text).font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// Your message, the agent's reply, its folded work or a note.
struct EntryRow: View {
    let entry: Turns.Entry
    let speaker: String

    var body: some View {
        switch entry {
        case .user(let item), .note(let item): ItemRow(item: item)
        case .reply(let item):
            VStack(alignment: .leading, spacing: 6) {
                Text([speaker, item.at.flatMap(Turns.time)].compactMap { $0 }.joined(separator: " · "))
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                MarkdownText(text: item.text ?? "")
            }
            .padding(.top, 4)
        case .progress(let item): MarkdownText(text: item.text ?? "", tone: .progress)
        case .work(let work): WorkRow(work: work)
        }
    }
}

/// The steps between two messages in one quiet line, "Worked 6m · Tests passed"; tapped, the steps.
struct WorkRow: View {
    let work: Turns.Work
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button { withAnimation(.snappy) { expanded.toggle() } } label: {
                HStack(spacing: 6) {
                    Image(systemName: expanded ? "chevron.down" : "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .frame(width: 14)
                    if work.running { ProgressView().controlSize(.mini) }
                    Text(line).lineLimit(1)
                    Spacer(minLength: 0)
                }
                .font(.footnote)
                .foregroundStyle(.secondary)
                .frame(minHeight: 36)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityHint(expanded ? "Hides the steps" : "Shows the \(work.items.count) steps")
            if expanded {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(work.items, id: \.id) { ItemRow(item: $0) }
                }
                .padding(.leading, 20)
                .padding(.top, 2)
                .padding(.bottom, 6)
            }
        }
    }

    /// "Worked 54s · 1 file +48 −6 · Tests passed", the results in their colours.
    private var line: AttributedString {
        var out = AttributedString(work.running ? "Working" : work.seconds.map { "Worked \(Turns.duration($0))" } ?? "Worked")
        for tag in work.tags {
            out += AttributedString(" · ")
            var t = AttributedString(tag.text)
            if tag.tone == .good { t.foregroundColor = .done } else if tag.tone == .bad { t.foregroundColor = .red }
            out += t
            if let added = tag.added { var a = AttributedString(" +\(added)"); a.foregroundColor = .done; out += a }
            if let removed = tag.removed, removed > 0 { var r = AttributedString(" −\(removed)"); r.foregroundColor = .red; out += r }
        }
        return out
    }
}

/// "1 file +48 −6", "Tests passed", "PR #49".
struct TagRow: View {
    let tags: [Turns.Tag]

    var body: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: 6) { ForEach(tags, id: \.text) { TagChip(tag: $0) } }
            HStack(spacing: 6) { ForEach(tags.prefix(2), id: \.text) { TagChip(tag: $0) } }
            HStack(spacing: 6) { ForEach(tags.prefix(1), id: \.text) { TagChip(tag: $0) } }
        }
    }
}

struct TagChip: View {
    let tag: Turns.Tag

    var body: some View {
        HStack(spacing: 3) {
            Text(tag.text)
            if let added = tag.added { Text("+\(added)").foregroundStyle(Color.done) }
            if let removed = tag.removed, removed > 0 { Text("−\(removed)").foregroundStyle(.red) }
        }
        .font(.caption)
        .lineLimit(1)
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .background(background, in: Capsule())
        .foregroundStyle(tag.tone == .good ? Color.done : tag.tone == .bad ? Color.red : Color.primary)
    }

    private var background: Color {
        switch tag.tone {
        case .good: Color.done.opacity(0.14)
        case .bad: Color.red.opacity(0.12)
        case .plain: Color(.tertiarySystemFill)
        }
    }
}

/// One entry: your message, Claude's reply, a tool call or a note.
struct ItemRow: View {
    let item: Conversation.Item
    @State private var expanded = false

    var body: some View {
        switch item.kind {
        case "user":
            Text(item.text ?? "")
                .font(.system(size: 15))
                .lineSpacing(3)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(Color.accentColor, in: UnevenRoundedRectangle(
                    topLeadingRadius: 18, bottomLeadingRadius: 18, bottomTrailingRadius: 4, topTrailingRadius: 18, style: .continuous))
                .foregroundStyle(.white)
                .frame(maxWidth: .infinity, alignment: .trailing)
                .padding(.leading, 56)
                .textSelection(.enabled)
        case "assistant":
            MarkdownText(text: item.text ?? "")
        case "tool":
            tool
        default:
            Text(item.text ?? "")
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity)
                .multilineTextAlignment(.center)
        }
    }

    private var tool: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button { withAnimation(.snappy) { expanded.toggle() } } label: {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Image(systemName: Self.icon(item.tool ?? ""))
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .frame(width: 18)
                    Text(item.text ?? item.tool ?? "Tool")
                        .font(.subheadline)
                        .foregroundStyle(.primary)
                        .lineLimit(expanded ? nil : 2)
                        .multilineTextAlignment(.leading)
                    Spacer(minLength: 4)
                    if !item.done {
                        ProgressView().controlSize(.mini)
                    } else if item.failed {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(.red).font(.footnote)
                    }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityHint(expanded ? "Hides the details" : "Shows the input and result")
            if expanded {
                if let input = item.input, !input.isEmpty { CodeBox(title: "Input", text: input) }
                if let output = item.output { CodeBox(title: item.failed ? "Failed" : "Result", text: output.isEmpty ? "(nothing)" : output) }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    static func icon(_ tool: String) -> String {
        switch tool {
        case "Bash": "terminal"
        case "Read": "doc.text"
        case "Edit", "MultiEdit", "Write", "NotebookEdit": "pencil"
        case "Grep", "Glob": "magnifyingglass"
        case "WebFetch", "WebSearch": "globe"
        case "Task", "Agent": "person.2"
        case "TodoWrite": "checklist"
        case "AskUserQuestion": "questionmark.bubble"
        case "ExitPlanMode": "list.bullet.clipboard"
        default: tool.hasPrefix("mcp__") ? "puzzlepiece.extension" : "wrench.and.screwdriver"
        }
    }
}

/// A tool's input or result: monospaced, scrolling sideways instead of wrapping.
struct CodeBox: View {
    let title: String
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption.weight(.semibold)).foregroundStyle(.secondary).textCase(.uppercase)
            ScrollView(.horizontal, showsIndicators: false) {
                Text(text)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(Color(white: 0.9))
                    .fixedSize(horizontal: true, vertical: false)
                    .padding(10)
                    .textSelection(.enabled)
            }
            .background(Color.codeBackground, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        }
    }
}

/// Where the agent is now, under the last message.
struct StatusLine: View {
    let status: SessionStatus

    var body: some View {
        HStack(spacing: 8) {
            if status.text == "Working" { ProgressView().controlSize(.small) } else { Image(systemName: status.symbol) }
            Text(status.text)
        }
        .font(.footnote.weight(.medium))
        .foregroundStyle(status.color)
        .frame(maxWidth: .infinity)
        .padding(.top, 4)
    }
}

/// The agent's screen like a photo of it: fitted to the phone's width at first, pinched to zoom,
/// double-tapped between fitted and full size. Text is laid out at full size, so it stays sharp.
struct TerminalView: View {
    let text: String
    var ansi: String?

    var body: some View {
        Group {
            if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                Text("The screen is blank.").font(.subheadline).foregroundStyle(Color(white: 0.6))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ZoomableText(text: text, ansi: ansi)
            }
        }
        .background(Color.codeBackground)
        .accessibilityIdentifier("terminal")
    }
}

private struct ZoomableText: UIViewRepresentable {
    let text: String
    let ansi: String?

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> Scroll {
        let scroll = Scroll()
        scroll.delegate = context.coordinator
        scroll.backgroundColor = .clear
        scroll.maximumZoomScale = 1.5
        scroll.contentInset = UIEdgeInsets(top: 12, left: 12, bottom: 12, right: 12)
        let label = UILabel()
        label.numberOfLines = 0
        label.lineBreakMode = .byClipping
        label.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        label.textColor = UIColor(white: 0.9, alpha: 1)
        label.isAccessibilityElement = true
        scroll.addSubview(label)
        context.coordinator.label = label
        let tap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.doubleTap(_:)))
        tap.numberOfTapsRequired = 2
        scroll.addGestureRecognizer(tap)
        scroll.onLayout = { [weak coordinator = context.coordinator] in coordinator?.fit($0) }
        return scroll
    }

    func updateUIView(_ scroll: Scroll, context: Context) {
        context.coordinator.show(text, ansi: ansi, in: scroll)
    }

    final class Scroll: UIScrollView {
        var onLayout: ((UIScrollView) -> Void)?
        override func layoutSubviews() {
            super.layoutSubviews()
            onLayout?(self)
        }
    }

    final class Coordinator: NSObject, UIScrollViewDelegate {
        var label: UILabel?
        private var shown: (String, String?)?
        private var fitted = false
        private var width: CGFloat = 0

        func viewForZooming(in scrollView: UIScrollView) -> UIView? { label }

        /// New text keeps the zoom; one read while at the bottom stays at the bottom.
        func show(_ text: String, ansi: String?, in scroll: UIScrollView) {
            guard let label, shown.map({ $0 != (text, ansi) }) ?? true else { return }
            shown = (text, ansi)
            let atBottom = !fitted || scroll.contentOffset.y >= bottom(scroll) - 24
            let zoom = scroll.zoomScale
            scroll.zoomScale = 1
            if let ansi { label.attributedText = Self.coloured(ansi, font: label.font) } else { label.text = text }
            label.accessibilityLabel = text
            let size = label.sizeThatFits(CGSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude))
            label.frame = CGRect(origin: .zero, size: size)
            scroll.contentSize = size
            scroll.zoomScale = zoom
            fitted = false
            fit(scroll)
            if atBottom { scroll.contentOffset.y = bottom(scroll) }
        }

        /// The screen's own colours on the dark background; dim text is drawn at half strength.
        static func coloured(_ ansi: String, font: UIFont) -> NSAttributedString {
            let out = NSMutableAttributedString()
            let base = UIColor(white: 0.9, alpha: 1)
            let bold = UIFont.monospacedSystemFont(ofSize: font.pointSize, weight: .bold)
            for run in Ansi.parse(ansi) {
                var fg = run.style.fg.map(Self.color) ?? base
                var bg = run.style.bg.map(Self.color)
                if run.style.inverse { (fg, bg) = (bg ?? UIColor(Color.codeBackground), fg) }
                if run.style.dim { fg = fg.withAlphaComponent(0.55) }
                var attrs: [NSAttributedString.Key: Any] = [.font: run.style.bold ? bold : font, .foregroundColor: fg]
                if let bg { attrs[.backgroundColor] = bg }
                if run.style.underline { attrs[.underlineStyle] = NSUnderlineStyle.single.rawValue }
                out.append(NSAttributedString(string: run.text, attributes: attrs))
            }
            return out
        }

        static func color(_ c: Ansi.Color) -> UIColor {
            UIColor(red: CGFloat(c.r) / 255, green: CGFloat(c.g) / 255, blue: CGFloat(c.b) / 255, alpha: 1)
        }

        /// The smallest zoom shows the whole width; the first layout starts there.
        func fit(_ scroll: UIScrollView) {
            guard let label, label.bounds.width > 0, scroll.bounds.width > 0 else { return }
            let available = scroll.bounds.width - scroll.contentInset.left - scroll.contentInset.right
            let minimum = min(1, available / label.bounds.width)
            guard !fitted || scroll.bounds.width != width else { return }
            let wasAtMinimum = scroll.zoomScale <= scroll.minimumZoomScale + 0.001
            scroll.minimumZoomScale = minimum
            if !fitted || wasAtMinimum || scroll.zoomScale < minimum { scroll.zoomScale = minimum }
            if !fitted { scroll.contentOffset = CGPoint(x: -scroll.contentInset.left, y: bottom(scroll)) }
            fitted = true
            width = scroll.bounds.width
        }

        @objc func doubleTap(_ g: UITapGestureRecognizer) {
            guard let scroll = g.view as? UIScrollView, let label else { return }
            if scroll.zoomScale > scroll.minimumZoomScale + 0.01 {
                scroll.setZoomScale(scroll.minimumZoomScale, animated: true)
            } else {
                let at = g.location(in: label)
                let w = scroll.bounds.width, h = scroll.bounds.height
                scroll.zoom(to: CGRect(x: at.x - w / 2, y: at.y - h / 2, width: w, height: h), animated: true)
            }
        }

        private func bottom(_ scroll: UIScrollView) -> CGFloat {
            max(-scroll.contentInset.top, scroll.contentSize.height - scroll.bounds.height + scroll.contentInset.bottom)
        }
    }
}

/// The agent's answer in reading type, or a note on its way there in smaller, quieter type.
enum ReplyTone { case answer, progress }

/// Claude's Markdown, enough of it for a phone: paragraphs with inline styles, headings, nested
/// lists, code blocks and tables. Code scrolls sideways; so does a table wider than four columns.
struct MarkdownText: View {
    let text: String
    var tone: ReplyTone = .answer
    // Smaller than the system body, so a phone line holds ~55 characters; it still follows Dynamic Type.
    @ScaledMetric(relativeTo: .subheadline) private var answerSize: CGFloat = 15
    @ScaledMetric(relativeTo: .subheadline) private var progressSize: CGFloat = 14
    @ScaledMetric(relativeTo: .headline) private var headingSize: CGFloat = 16
    @ScaledMetric(relativeTo: .footnote) private var sectionSize: CGFloat = 13

    var body: some View {
        let answer = tone == .answer
        let size = answer ? answerSize : progressSize
        let color = answer ? Color.replyText : Color.secondary
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(Markdown.blocks(text).enumerated()), id: \.offset) { i, block in
                Group {
                    switch block {
                    case .paragraph(let t):
                        Text(Self.inline(t, size: size, strong: answer ? .primary : nil)).foregroundStyle(color)
                    case .heading(let level, let h):
                        if answer && level <= 2 {
                            Text(Self.inline(h, size: headingSize)).font(.system(size: headingSize, weight: .semibold)).lineSpacing(2)
                        } else {
                            Text(Self.inline(h.uppercased(), size: sectionSize))
                                .font(.system(size: sectionSize, weight: .semibold)).tracking(0.4).foregroundStyle(.secondary)
                        }
                    case .list(let items):
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                                let bullet = item.marker == "•"
                                HStack(alignment: .firstTextBaseline, spacing: 0) {
                                    Text(bullet && item.level > 0 ? "◦" : item.marker)
                                        .foregroundStyle(.secondary)
                                        .frame(width: bullet ? 14 : 22, alignment: .leading)
                                    Text(Self.inline(item.text, size: size, strong: answer ? .primary : nil))
                                        .foregroundStyle(item.level > 0 && answer ? Color.replyDim : color)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                }
                                .padding(.leading, CGFloat(18 * item.level))
                            }
                        }
                    case .code(let c): CodeBox(title: "", text: c)
                    case .table(let t):
                        if let table = MarkdownTable.parse(t) { TableView(table: table) } else { Text(t).foregroundStyle(color) }
                    case .rule: Divider().padding(.vertical, 4)
                    }
                }
                .font(.system(size: size))
                .lineSpacing(answer ? 4 : 3)
                .padding(.top, i == 0 ? 0 : Self.isHeading(block) ? 14 : 8)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
    }

    private static func isHeading(_ b: Markdown.Block) -> Bool {
        if case .heading = b { return true }
        return false
    }

    /// Bold brighter, code smaller and monospaced on a quiet tint, links in the accent.
    /// `strong`: bold's colour; nil keeps the text's own, as in a note.
    static func inline(_ s: String, size: CGFloat = 15, strong: Color? = .primary) -> AttributedString {
        var out = (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(s)
        var code: [Range<AttributedString.Index>] = []
        for run in out.runs {
            guard let intent = run.inlinePresentationIntent else { continue }
            if intent.contains(.code) {
                out[run.range].font = .system(size: size - 2, design: .monospaced)
                out[run.range].backgroundColor = .codeFill
                code.append(run.range)
            } else if intent.contains(.stronglyEmphasized) {
                out[run.range].font = .system(size: size, weight: .semibold)
                if let strong { out[run.range].foregroundColor = strong }
            }
        }
        // Thin spaces give the tint a little room either side.
        for range in code.reversed() {
            var pad = AttributedString("\u{2009}")
            pad.backgroundColor = .codeFill
            out.insert(pad, at: range.upperBound)
            out.insert(pad, at: range.lowerBound)
        }
        return out
    }
}
