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
            terminal = switch source {
            case .session(let id): try await hub.output(id, visible: true).text
            case .agent(let machine, let name): try await hub.agentOutput(machine: machine, agent: name, visible: true).text
            }
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
    let ref: ConversationRef
    @State private var convo: ConversationModel
    @State private var tab = Tab.conversation

    enum Tab: String, CaseIterable {
        case conversation = "Conversation"
        case terminal = "Terminal"
    }

    init(ref: ConversationRef) {
        self.ref = ref
        _convo = State(initialValue: ConversationModel(ref.source))
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
            TerminalView(text: convo.terminal)
        } else if let error = convo.terminalError {
            Waiting(symbol: "exclamationmark.triangle", text: "Couldn't read the screen: \(error)", busy: false)
        } else {
            Waiting(symbol: "terminal", text: "Loading the screen…", busy: true)
        }
    }

    private var feed: some View {
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
                ForEach(convo.conversation.items, id: \.id) { ItemRow(item: $0) }
                if convo.conversation.isLoaded {
                    if let status { StatusLine(status: status) }
                    if let error = convo.error { Text(error).font(.footnote).foregroundStyle(.orange) }
                }
            }
            .padding(.horizontal, 16)
            .padding(.bottom, 24)
        }
        .defaultScrollAnchor(.bottom)
        .accessibilityIdentifier("conversation")
    }

    private var status: SessionStatus? {
        switch ref.source {
        case .session(let id): model.sessions.first { $0.id == id }.map(SessionStatus.init)
        case .agent(let machine, let name):
            model.agents.first { $0.machineId == machine && $0.name == name }.map { SessionStatus(agentStatus: $0.agentStatus) }
        }
    }
}

/// The latest few items, for a session or agent page.
struct ConversationPreview: View {
    let convo: ConversationModel
    var count = 6

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if !convo.conversation.isLoaded {
                ConversationSkeleton(rows: 2)
            } else if convo.conversation.items.isEmpty {
                Text("Nothing said yet.").font(.subheadline).foregroundStyle(.secondary)
            }
            ForEach(convo.conversation.items.suffix(count), id: \.id) { ItemRow(item: $0) }
        }
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

/// One entry: your message, Claude's reply, a tool call or a note.
struct ItemRow: View {
    let item: Conversation.Item
    @State private var expanded = false

    var body: some View {
        switch item.kind {
        case "user":
            Text(item.text ?? "")
                .padding(.horizontal, 13)
                .padding(.vertical, 9)
                .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                .foregroundStyle(.white)
                .frame(maxWidth: .infinity, alignment: .trailing)
                .padding(.leading, 40)
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

    var body: some View {
        Group {
            if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                Text("The screen is blank.").font(.subheadline).foregroundStyle(Color(white: 0.6))
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ZoomableText(text: text)
            }
        }
        .background(Color.codeBackground)
        .accessibilityIdentifier("terminal")
    }
}

private struct ZoomableText: UIViewRepresentable {
    let text: String

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
        context.coordinator.show(text, in: scroll)
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
        private var fitted = false
        private var width: CGFloat = 0

        func viewForZooming(in scrollView: UIScrollView) -> UIView? { label }

        /// New text keeps the zoom; one read while at the bottom stays at the bottom.
        func show(_ text: String, in scroll: UIScrollView) {
            guard let label, label.text != text else { return }
            let atBottom = !fitted || scroll.contentOffset.y >= bottom(scroll) - 24
            let zoom = scroll.zoomScale
            scroll.zoomScale = 1
            label.text = text
            label.accessibilityLabel = text
            let size = label.sizeThatFits(CGSize(width: CGFloat.greatestFiniteMagnitude, height: .greatestFiniteMagnitude))
            label.frame = CGRect(origin: .zero, size: size)
            scroll.contentSize = size
            scroll.zoomScale = zoom
            fitted = false
            fit(scroll)
            if atBottom { scroll.contentOffset.y = bottom(scroll) }
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

/// Claude's Markdown, enough of it for a phone: paragraphs with inline styles, headings, lists,
/// code blocks and tables. Code and tables scroll sideways.
struct MarkdownText: View {
    let text: String

    enum Block {
        case text(String)
        case heading(String)
        case code(String)
        case table(String)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(Array(Self.blocks(text).enumerated()), id: \.offset) { _, block in
                switch block {
                case .text(let t): Text(Self.inline(t))
                case .heading(let h): Text(Self.inline(h)).font(.headline)
                case .code(let c): CodeBox(title: "", text: c)
                case .table(let t):
                    ScrollView(.horizontal, showsIndicators: false) {
                        Text(t).font(.system(size: 12, design: .monospaced)).fixedSize(horizontal: true, vertical: false)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
    }

    static func blocks(_ text: String) -> [Block] {
        var out: [Block] = []
        var para: [String] = []
        var code: [String]?
        var table: [String] = []
        func flush() {
            if !para.isEmpty { out.append(.text(para.joined(separator: "\n"))); para = [] }
            if !table.isEmpty { out.append(.table(table.joined(separator: "\n"))); table = [] }
        }
        for line in text.components(separatedBy: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") {
                if let c = code { out.append(.code(c.joined(separator: "\n"))); code = nil } else { flush(); code = [] }
            } else if code != nil {
                code?.append(line)
            } else if trimmed.hasPrefix("|") {
                if !para.isEmpty { out.append(.text(para.joined(separator: "\n"))); para = [] }
                table.append(trimmed)
            } else if trimmed.hasPrefix("#") {
                flush()
                out.append(.heading(trimmed.drop { $0 == "#" }.trimmingCharacters(in: .whitespaces)))
            } else if trimmed.isEmpty {
                flush()
            } else {
                if !table.isEmpty { out.append(.table(table.joined(separator: "\n"))); table = [] }
                para.append(bullet(line))
            }
        }
        if let c = code { out.append(.code(c.joined(separator: "\n"))) }
        flush()
        return out
    }

    /// `- item` and `* item` as bullets; numbered items stay as written.
    private static func bullet(_ line: String) -> String {
        let indent = line.prefix { $0 == " " }
        let rest = line.dropFirst(indent.count)
        guard rest.hasPrefix("- ") || rest.hasPrefix("* ") else { return line }
        return indent + "• " + rest.dropFirst(2)
    }

    static func inline(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(s)
    }
}
