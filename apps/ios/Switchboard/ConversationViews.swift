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
    /// Why there's no conversation, e.g. a Codex agent or an older hub; the terminal stands in.
    private(set) var unavailable: String?
    private(set) var terminal = ""
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
    private func loadTerminal(_ hub: Hub) async {
        let text: String? = switch source {
        case .session(let id): try? await hub.output(id, visible: true).text
        case .agent(let machine, let name): try? await hub.agentOutput(machine: machine, agent: name, visible: true).text
        }
        if let text { terminal = text }
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
            Picker("View", selection: $tab) {
                ForEach(Tab.allCases, id: \.self) { Text($0.rawValue) }
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
            if tab == .terminal || convo.unavailable != nil {
                if tab == .conversation, let why = convo.unavailable {
                    Text("No conversation to show (\(why)), so here's the terminal.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 16)
                        .padding(.bottom, 8)
                }
                TerminalView(text: convo.terminal)
            } else {
                feed
            }
        }
        .background(Color(.systemGroupedBackground))
        .navigationTitle(ref.title)
        .navigationBarTitleDisplayMode(.inline)
        .onChange(of: tab, initial: true) { _, t in convo.wantsTerminal = t == .terminal }
        .task(id: model.wakes) { await convo.follow(model) }
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
                    ProgressView().frame(maxWidth: .infinity).padding(.top, 40)
                } else if convo.conversation.items.isEmpty {
                    Text("Nothing said yet.").foregroundStyle(.secondary).frame(maxWidth: .infinity).padding(.top, 40)
                }
                ForEach(convo.conversation.items, id: \.id) { ItemRow(item: $0) }
                if let status { StatusLine(status: status) }
                if let error = convo.error { Text(error).font(.footnote).foregroundStyle(.orange) }
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
            ForEach(convo.conversation.items.suffix(count), id: \.id) { ItemRow(item: $0) }
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

/// The agent's terminal as it is: no wrapping, so wide lines scroll sideways.
struct TerminalView: View {
    let text: String

    var body: some View {
        ScrollView([.horizontal, .vertical]) {
            Text(text.isEmpty ? "No output yet." : text)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(Color(white: 0.9))
                .fixedSize(horizontal: true, vertical: false)
                .padding(12)
                .textSelection(.enabled)
        }
        .defaultScrollAnchor(.bottomLeading)
        .background(Color.codeBackground)
        .accessibilityIdentifier("terminal")
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
