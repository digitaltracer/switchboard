import SwiftUI
import SwitchboardKit

/// One agent's conversation, and its terminal, followed while a screen shows them. Pages that
/// don't draw the terminal pass false for `drawsTerminal`, and its screen is never read.
@MainActor
@Observable
final class ConversationModel {
    let source: ConversationSource
    private let drawsTerminal: Bool
    private(set) var conversation = Conversation()
    /// Why there's no conversation, e.g. no log yet or an older hub; the terminal stands in.
    private(set) var unavailable: String?
    /// The hub says the session has ended: nothing more to follow, and its screen isn't read.
    private(set) var ended = false
    private(set) var terminal = ""
    /// The same screen with its colours, when the hub sends them.
    private(set) var terminalAnsi: String?
    /// False until the screen has been read once.
    private(set) var terminalLoaded = false
    private(set) var terminalError: String?
    private(set) var error: String?
    private(set) var loadingEarlier = false
    /// The terminal is read only while it's on screen or standing in.
    private(set) var wantsTerminal = false
    /// Wakes the screen loop when the screen is shown.
    private let screenBell = Bell()
    private var backoff = UnavailableBackoff()

    init(_ source: ConversationSource, drawsTerminal: Bool = true) {
        self.source = source
        self.drawsTerminal = drawsTerminal
    }

    /// Follows until cancelled or the app leaves the foreground: the conversation as the hub
    /// sends more of it, and the screen each second while it's shown or standing in.
    func follow(_ model: AppModel) async {
        async let talk: Void = followConversation(model)
        async let look: Void = followScreen(model)
        _ = await (talk, look)
    }

    private func followConversation(_ model: AppModel) async {
        await Follow.repeating(going: { model.active && !ended }) {
            let ok = await refresh(model.hub, wait: true)
            let extra = backoff.extra(unavailable: unavailable != nil)
            if extra > .zero { try? await Task.sleep(for: extra) }
            return ok
        }
    }

    private func followScreen(_ model: AppModel) async {
        await Follow.screens(bell: screenBell, shown: { Follow.readsScreen(draws: drawsTerminal, wanted: wantsTerminal, unavailable: unavailable != nil) },
                             going: { model.active && !ended }) {
            await loadTerminal(model.hub)
        }
    }

    /// The Screen tab opened or closed; opening it reads the screen at once.
    func showTerminal(_ shown: Bool) {
        wantsTerminal = shown
        if shown { screenBell.ring() }
    }

    /// False when it should be tried again later rather than at once.
    @discardableResult
    func refresh(_ hub: Hub?, wait: Bool = false) async -> Bool {
        guard let hub else { return false }
        let wasAvailable = unavailable == nil
        // The screen stands in for a conversation there's none of.
        defer { if wasAvailable, unavailable != nil { screenBell.ring() } }
        do {
            if conversation.isLoaded {
                let after = conversation.end
                conversation.append(try await Follow.page(hub, source, after: after, before: nil, wait: wait), askedAfter: after)
            } else {
                conversation.replace(with: try await Follow.page(hub, source, after: nil, before: nil))
            }
            unavailable = nil
            error = nil
            return true
        } catch let e as HubError where e.code == "session_not_running" {
            ended = true
        } catch let e as HubError where e.code == "http_404" {
            // A hub older than this app has no conversations to give.
            unavailable = "the hub needs updating"
        } catch let e as HubError where ["bad_request", "not_found"].contains(e.code ?? "") {
            unavailable = e.localizedDescription
        } catch {
            if !Task.isCancelled { self.error = error.localizedDescription }
        }
        return false
    }

    func loadEarlier(_ hub: Hub) async {
        guard let start = conversation.start, start > 0, !loadingEarlier else { return }
        loadingEarlier = true
        defer { loadingEarlier = false }
        do {
            conversation.prepend(try await Follow.page(hub, source, after: nil, before: start))
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// Only the screen: reading further back would scroll the agent's terminal on the Mac. A hub
    /// busy with other screen reads skips this one.
    @discardableResult
    func loadTerminal(_ hub: Hub?) async -> Bool {
        guard let hub else { return false }
        do {
            let out = try await Follow.screen(hub, source)
            terminal = out.text
            terminalAnsi = out.ansi
            terminalLoaded = true
            terminalError = nil
            return true
        } catch let e as HubError where e.code == "busy" {
            return true
        } catch let e as HubError where e.code == "session_not_running" {
            ended = true
            return false
        } catch {
            if !Task.isCancelled { terminalError = error.localizedDescription }
            return false
        }
    }
}

/// The whole conversation, newest at the bottom, following as it grows.
struct ConversationFeed: View {
    @Environment(AppModel.self) private var model
    let convo: ConversationModel
    let speaker: String
    /// Where the agent is now, under the last message.
    let status: SessionStatus?
    /// Whether the conversation is scrolled to its latest message.
    @State private var atEnd = true

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    if convo.conversation.hasEarlier {
                        Button {
                            Task { if let hub = model.hub { await convo.loadEarlier(hub) } }
                        } label: {
                            if convo.loadingEarlier { ProgressView() } else { Text("Show earlier messages").font(Face.geist(13.5, .medium)).foregroundStyle(Ink.ac) }
                        }
                        .frame(maxWidth: .infinity, minHeight: 44)
                    }
                    if convo.ended {
                        StateRow(text: "This session has ended, so there's nothing more to show.")
                    } else if let why = convo.unavailable {
                        StateRow(text: "No conversation to show (\(why)). Its screen is under Screen.")
                    } else if !convo.conversation.isLoaded {
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
                        ForEach(convo.conversation.queued, id: \.id) { QueuedRow(item: $0, speaker: speaker) }
                        if let error = convo.error { Note(text: error, color: Ink.warn) }
                    }
                    Color.clear.frame(height: 1).id(Self.end)
                }
                .padding(.horizontal, 20)
                .padding(.top, 8)
                .padding(.bottom, 24)
            }
            .defaultScrollAnchor(.bottom)
            .modifier(TracksEnd(atEnd: $atEnd))
            // The first page replaces the loading placeholder, whose offset leaves the lazy stack showing
            // nothing until scrolled; new messages follow only if you were at the end.
            .onChange(of: convo.conversation.items.last?.id, initial: true) { old, _ in
                if old == nil || atEnd { proxy.scrollTo(Self.end, anchor: .bottom) }
            }
            .onChange(of: convo.conversation.queued.last?.id) {
                if atEnd { proxy.scrollTo(Self.end, anchor: .bottom) }
            }
            .overlay(alignment: .bottom) {
                if !atEnd {
                    Button {
                        withAnimation(.snappy) { proxy.scrollTo(Self.end, anchor: .bottom) }
                    } label: {
                        Image(systemName: "arrow.down")
                            .font(.body.weight(.semibold))
                            .frame(width: 40, height: 40)
                            .background(Ink.sf, in: Circle())
                            .overlay(Circle().strokeBorder(Ink.line))
                            .shadow(color: .black.opacity(0.12), radius: 6, y: 2)
                            .frame(width: 44, height: 44)
                            .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(Ink.ink)
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
                        Text(MarkdownText.inline(cell)).font(Face.geist(13, .semibold)).foregroundStyle(Ink.mut)
                    }
                }
                .padding(.vertical, 6)
            }
            ForEach(Array(table.rows.enumerated()), id: \.offset) { i, row in
                if i > 0 || !table.header.isEmpty { Hairline() }
                WeightedRow(weights: table.weights) {
                    ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                        Text(MarkdownText.inline(cell)).font(Face.sub)
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
/// At most `height` tall; cut-off content fades out.
struct Clamped<Content: View>: View {
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
struct TracksEnd: ViewModifier {
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
        .foregroundStyle(Ink.fill)
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
            if busy { ProgressView() } else { Image(systemName: symbol).font(.title2).foregroundStyle(Ink.mut) }
            Text(text).font(Face.sub).foregroundStyle(Ink.mut).multilineTextAlignment(.center)
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
        case .peer(let item): PeerBubble(item: item)
        case .reply(let item):
            VStack(alignment: .leading, spacing: 6) {
                Text([speaker, item.at.flatMap(Turns.time)].compactMap { $0 }.joined(separator: " · ").uppercased())
                    .font(Face.section)
                    .tracking(0.84)
                    .foregroundStyle(Ink.mut)
                MarkdownText(text: item.text ?? "")
            }
            .padding(.top, 4)
        case .progress(let item): MarkdownText(text: item.text ?? "", tone: .progress)
        case .work(let work): WorkRow(work: work)
        }
    }
}

/// A message another terminal sent the agent: who it's from, then what it said, outlined apart
/// from yours.
struct PeerBubble: View {
    let item: Conversation.Item

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(["From \(item.from ?? "another terminal")", item.at.flatMap(Turns.time)].compactMap { $0 }.joined(separator: " · ").uppercased())
                .font(Face.section)
                .tracking(0.84)
                .foregroundStyle(Ink.mut)
            MarkdownText(text: item.text ?? "")
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .strokeBorder(Ink.line, style: StrokeStyle(lineWidth: 1, dash: [4, 3])))
                .padding(.trailing, 48)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 4)
        .accessibilityElement(children: .combine)
    }
}

/// A message waiting for the agent to take it in: faded, with a clock and "Queued" under it.
struct QueuedRow: View {
    let item: Conversation.Item
    let speaker: String

    var body: some View {
        let mine = item.kind != "peer"
        VStack(alignment: mine ? .trailing : .leading, spacing: 6) {
            Group {
                if mine { MeBubble(text: item.text ?? "") } else { PeerBubble(item: item) }
            }
            .opacity(0.5)
            Label("Queued · \(speaker) reads it after this step", systemImage: "clock")
                .font(Face.meta)
                .foregroundStyle(Ink.mut)
        }
        .frame(maxWidth: .infinity, alignment: mine ? .trailing : .leading)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("queued")
    }
}

/// The steps between two messages in one quiet line, "Worked 6m · Tests passed"; tapped, the steps.
struct WorkRow: View {
    let work: Turns.Work
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button { withAnimation(.snappy) { expanded.toggle() } } label: {
                HStack(spacing: 8) {
                    if work.running { StatusGlyph(glyph: .work, size: 14) } else { Chevron(down: expanded).frame(width: 14) }
                    Text(line).lineLimit(1)
                    Spacer(minLength: 0)
                }
                .font(Face.sub)
                .foregroundStyle(work.running ? Ink.ac : Ink.sub)
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityHint(expanded ? "Hides the steps" : "Shows the \(work.items.count) steps")
            if expanded {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(work.items, id: \.id) { ItemRow(item: $0) }
                }
                .padding(.leading, 16)
                .overlay(alignment: .leading) { Rectangle().fill(Ink.line).frame(width: 1.5) }
                .padding(.leading, 8)
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
            if tag.tone == .good { t.foregroundColor = Ink.ok } else if tag.tone == .bad { t.foregroundColor = Ink.bad }
            out += t
            if let added = tag.added { var a = AttributedString(" +\(added)"); a.foregroundColor = Ink.ok; out += a }
            if let removed = tag.removed, removed > 0 { var r = AttributedString(" −\(removed)"); r.foregroundColor = Ink.bad; out += r }
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
            if let added = tag.added { Text("+\(added)").foregroundStyle(Ink.ok) }
            if let removed = tag.removed, removed > 0 { Text("−\(removed)").foregroundStyle(Ink.bad) }
        }
        .font(Face.geist(13, .medium, relativeTo: .footnote))
        .lineLimit(1)
        .padding(.horizontal, 10)
        .frame(height: 28)
        .background(Ink.fill, in: Capsule())
        .foregroundStyle(tag.tone == .good ? Ink.ok : tag.tone == .bad ? Ink.bad : Ink.sub)
    }
}

/// One entry: your message, Claude's reply, a tool call or a note.
struct ItemRow: View {
    let item: Conversation.Item
    @State private var expanded = false

    var body: some View {
        switch item.kind {
        case "user":
            MeBubble(text: item.text ?? "")
        case "peer":
            PeerBubble(item: item)
        case "assistant":
            MarkdownText(text: item.text ?? "")
        case "tool":
            tool
        default:
            Text(item.text ?? "")
                .font(Face.meta)
                .foregroundStyle(Ink.mut)
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
                        .foregroundStyle(Ink.mut)
                        .frame(width: 18)
                    Text(item.text ?? item.tool ?? "Tool")
                        .font(Face.sub)
                        .foregroundStyle(Ink.sub)
                        .lineLimit(expanded ? nil : 2)
                        .multilineTextAlignment(.leading)
                    Spacer(minLength: 4)
                    if !item.done {
                        ProgressView().controlSize(.mini)
                    } else if item.failed {
                        Image(systemName: "xmark.circle.fill").foregroundStyle(Ink.bad).font(.footnote)
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
            Text(title.uppercased()).font(Face.section).tracking(0.84).foregroundStyle(Ink.mut)
            ScrollView(.horizontal, showsIndicators: false) {
                Text(text)
                    .font(Face.mono(12))
                    .foregroundStyle(Ink.codeText)
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
            StatusGlyph(glyph: status.glyph, size: 14)
            Text(status.text)
            Spacer(minLength: 0)
        }
        .font(Face.geist(13.5, .medium, relativeTo: .footnote))
        .foregroundStyle(status.color)
        .frame(minHeight: 44)
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
                Text("The screen is blank.").font(Face.sub).foregroundStyle(Ink.codeDim)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ZoomableText(text: text, ansi: ansi)
            }
        }
        .background(Ink.term)
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
    @ScaledMetric(relativeTo: .body) private var answerSize: CGFloat = 16
    @ScaledMetric(relativeTo: .subheadline) private var progressSize: CGFloat = 14.5
    @ScaledMetric(relativeTo: .headline) private var headingSize: CGFloat = 16
    @ScaledMetric(relativeTo: .footnote) private var sectionSize: CGFloat = 13

    var body: some View {
        let answer = tone == .answer
        let size = answer ? answerSize : progressSize
        let color = answer ? Color.replyText : Ink.sub
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(Markdown.blocks(text).enumerated()), id: \.offset) { i, block in
                Group {
                    switch block {
                    case .paragraph(let t):
                        Text(Self.inline(t, size: size, strong: answer ? Ink.ink : nil)).foregroundStyle(color)
                    case .heading(let level, let h):
                        if answer && level <= 2 {
                            Text(Self.inline(h, size: headingSize)).font(Face.fixed(headingSize, .semibold)).lineSpacing(2)
                        } else {
                            Text(Self.inline(h.uppercased(), size: sectionSize))
                                .font(Face.fixed(sectionSize, .medium)).tracking(0.8).foregroundStyle(Ink.mut)
                        }
                    case .list(let items):
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(Array(items.enumerated()), id: \.offset) { _, item in
                                let bullet = item.marker == "•"
                                HStack(alignment: .firstTextBaseline, spacing: 0) {
                                    Text(bullet && item.level > 0 ? "◦" : item.marker)
                                        .foregroundStyle(Ink.mut)
                                        .frame(width: bullet ? 14 : 22, alignment: .leading)
                                    Text(Self.inline(item.text, size: size, strong: answer ? Ink.ink : nil))
                                        .foregroundStyle(item.level > 0 && answer ? Color.replyDim : color)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                }
                                .padding(.leading, CGFloat(18 * item.level))
                            }
                        }
                    case .code(let c): CodeBox(title: "", text: c)
                    case .table(let t):
                        if let table = MarkdownTable.parse(t) { TableView(table: table) } else { Text(t).foregroundStyle(color) }
                    case .rule: Hairline().padding(.vertical, 4)
                    }
                }
                .font(Face.fixed(size))
                .lineSpacing(answer ? 5 : 3)
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
    static func inline(_ s: String, size: CGFloat = 15, strong: Color? = Ink.ink) -> AttributedString {
        var out = (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(s)
        var code: [Range<AttributedString.Index>] = []
        for run in out.runs {
            guard let intent = run.inlinePresentationIntent else { continue }
            if intent.contains(.code) {
                out[run.range].font = Face.fixed(size - 2, mono: true)
                out[run.range].backgroundColor = .codeFill
                code.append(run.range)
            } else if intent.contains(.stronglyEmphasized) {
                out[run.range].font = Face.fixed(size, .semibold)
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
