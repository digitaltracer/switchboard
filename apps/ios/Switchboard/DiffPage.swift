import SwiftUI
import SwitchboardKit

/// A changed file of a session, as the Changes list opens it; `paths` lets the page step through them all.
struct DiffRef: Hashable {
    let sessionId: String
    let paths: [String]
    var index: Int
}

/// One changed file's unified diff, read-only and masked. Long unchanged runs fold; a big file
/// loads a page of hunks at a time. Tapping a line number, then another, selects lines to ask
/// the agent about.
struct DiffPage: View {
    @Environment(AppModel.self) private var model
    @State var ref: DiffRef
    @State private var diff: Schemas.FileDiffView?
    @State private var error: String?
    @State private var loadingMore = false
    @State private var wrap = true
    /// Folds opened, by hunk and the fold's first line.
    @State private var open: [Int: Set<Int>] = [:]
    @State private var selection: Selection?
    @State private var asking = false

    struct Selection: Equatable {
        let hunk: Int
        var anchor: Int
        var end: Int
        var range: ClosedRange<Int> { min(anchor, end)...max(anchor, end) }
    }

    private var path: String { ref.paths[ref.index] }

    var body: some View {
        VStack(spacing: 0) {
        PageTop(back: "Changes", labelled: true) {
            HStack(spacing: 14) {
                Text("\(ref.index + 1) of \(ref.paths.count)").font(Face.meta).foregroundStyle(Ink.mut)
                Button { step(-1) } label: { Image(systemName: "chevron.up") }
                    .disabled(ref.index == 0).accessibilityLabel("Previous file")
                Button { step(1) } label: { Image(systemName: "chevron.down") }
                    .disabled(ref.index + 1 >= ref.paths.count).accessibilityLabel("Next file")
            }
            .font(.system(size: 16, weight: .medium))
            .foregroundStyle(Ink.ink)
        }
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                header
                if let diff {
                    if diff.withheld {
                        Note(text: "This file is the kind that holds secrets, such as keys or an env file, so it isn't shown on the phone.")
                            .padding(20)
                    } else if diff.binary {
                        Note(text: "Binary file\(diff.size.map { " · \(fileSize($0))" } ?? ""). Only its size is shown.")
                            .padding(20)
                    } else if diff.hunks.isEmpty {
                        Note(text: "No changes to show.").padding(20)
                    } else {
                        ForEach(Array(diff.hunks.enumerated()), id: \.offset) { h, hunk in
                            hunkView(h, hunk)
                        }
                        if diff.next != nil {
                            Button(loadingMore ? "Loading…" : "Show more of this file") { Task { await more() } }
                                .font(Face.geist(14, .medium)).foregroundStyle(Ink.ac)
                                .padding(20)
                                .disabled(loadingMore)
                        }
                    }
                    Note(text: "Secrets are masked before they leave your machine, as in all output.")
                        .padding(.horizontal, 20).padding(.vertical, 16)
                } else if let error {
                    Note(text: error, color: Ink.bad).padding(20)
                } else {
                    ProgressView().frame(maxWidth: .infinity).padding(40)
                }
            }
        }
        .safeAreaInset(edge: .bottom) { if selection != nil { selectionBar } }
        }
        .inboxPage()
        .task(id: ref.index) { await load() }
        .sheet(isPresented: $asking) {
            if let q = quote { AskSheet(sessionId: ref.sessionId, path: path, lines: q) { asking = false; selection = nil } }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(path).font(Face.mono(14, .medium)).foregroundStyle(Ink.ink).textSelection(.enabled)
            HStack(spacing: 6) {
                if let d = diff, !d.binary, !d.withheld {
                    Text("+\(d.added ?? 0)").foregroundStyle(Ink.ok)
                    Text("−\(d.deleted ?? 0)").foregroundStyle(Ink.bad)
                    Text("· \(d.hunkCount) \(d.hunkCount == 1 ? "hunk" : "hunks")").foregroundStyle(Ink.mut)
                }
                Spacer()
                if let d = diff, !d.binary, !d.withheld, !d.hunks.isEmpty {
                    Toggle("Wrap", isOn: $wrap).toggleStyle(.button).font(Face.geist(13)).tint(Ink.ac)
                }
            }
            .font(Face.mono(12.5))
        }
        .padding(.horizontal, 20)
        .padding(.top, 8)
        .padding(.bottom, 12)
    }

    @ViewBuilder
    private func hunkView(_ h: Int, _ hunk: Schemas.DiffHunkView) -> some View {
        Text(hunk.header)
            .font(Face.mono(11.5)).foregroundStyle(Ink.mut).lineLimit(1)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 12).padding(.vertical, 8)
            .background(Ink.fill)
        let rows = DiffFolding.rows(hunk.lines.map(\.kind), open: open[h] ?? [])
        let lines = VStack(alignment: .leading, spacing: 0) {
            ForEach(rows) { row in
                switch row {
                case .line(let i): lineView(h, i, hunk.lines[i])
                case .fold(let r):
                    Button("Show \(r.count) unchanged lines") { open[h, default: []].insert(r.lowerBound) }
                        .font(Face.geist(13, .medium)).foregroundStyle(Ink.ac)
                        .padding(.horizontal, 12).padding(.vertical, 9)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            if hunk.cut {
                Note(text: "This change is too long to show in full here.").padding(12)
            }
        }
        if wrap {
            lines
        } else {
            ScrollView(.horizontal, showsIndicators: false) { lines }
        }
    }

    private func lineView(_ h: Int, _ i: Int, _ l: Schemas.DiffLineView) -> some View {
        let selected = selection.map { $0.hunk == h && $0.range.contains(i) } ?? false
        let bg: Color = l.kind == "+" ? Ink.ok.opacity(0.12) : l.kind == "-" ? Ink.bad.opacity(0.12) : .clear
        return HStack(alignment: .firstTextBaseline, spacing: 0) {
            Button { tap(h, i) } label: {
                HStack(spacing: 0) {
                    Text(l.old.map(String.init) ?? "").frame(width: 34, alignment: .trailing)
                    Text(l.new.map(String.init) ?? "").frame(width: 34, alignment: .trailing)
                }
                .font(Face.mono(11)).foregroundStyle(selected ? Ink.ac : l.kind == "-" ? Ink.bad : Ink.mut)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Line \(l.new ?? l.old ?? 0)")
            Text(l.kind == " " ? " " : l.kind).font(Face.mono(12.5)).foregroundStyle(Ink.mut).frame(width: 16)
            Text(l.text.isEmpty ? " " : l.text)
                .font(Face.mono(12.5)).foregroundStyle(Ink.ink)
                .lineLimit(wrap ? nil : 1)
                .fixedSize(horizontal: !wrap, vertical: true)
                .frame(maxWidth: wrap ? .infinity : nil, alignment: .leading)
        }
        .padding(.vertical, 2)
        .padding(.trailing, 12)
        .background(bg)
        .overlay { if selected { Rectangle().strokeBorder(Ink.ac, lineWidth: 1.5) } }
    }

    private var selectionBar: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(selectionTitle).font(Face.geist(15, .medium)).foregroundStyle(Ink.ink)
                Text("Tap a line number, then another, to select").font(Face.geist(12.5)).foregroundStyle(Ink.mut)
            }
            Spacer()
            Button("Ask agent") { asking = true }
                .font(Face.geist(15, .semibold)).foregroundStyle(Ink.inv)
                .padding(.horizontal, 18).frame(height: 44)
                .background(Ink.ink, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .accessibilityIdentifier("askAgent")
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
        .background(Ink.sf)
        .overlay(alignment: .top) { Hairline() }
    }

    private var selectionTitle: String {
        guard let q = quote, let lo = q.map(\.0).min(), let hi = q.map(\.0).max() else { return "" }
        return lo == hi ? "Line \(lo) selected" : "Lines \(lo)–\(hi) selected"
    }

    /// The selected lines, numbered in the new file, or the old one for deleted lines.
    private var quote: [(Int, String)]? {
        guard let s = selection, let hunk = diff?.hunks[safe: s.hunk] else { return nil }
        return s.range.compactMap { i in
            hunk.lines[safe: i].map { (($0.new ?? $0.old).map(Int.init) ?? 0, $0.text) }
        }
    }

    /// The first tap selects a line; a second in the same change extends to it; a third starts over.
    private func tap(_ h: Int, _ i: Int) {
        if var s = selection, s.hunk == h, s.anchor == s.end {
            s.end = i
            selection = s
        } else if let s = selection, s.hunk == h, s.range == i...i {
            selection = nil
        } else {
            selection = Selection(hunk: h, anchor: i, end: i)
        }
    }

    private func step(_ by: Int) {
        let next = ref.index + by
        guard ref.paths.indices.contains(next) else { return }
        ref.index = next
        diff = nil
        selection = nil
        open = [:]
    }

    private func load() async {
        guard let hub = model.hub else { return }
        error = nil
        do {
            diff = try await hub.diff(ref.sessionId, path: path)
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func more() async {
        guard let hub = model.hub, let d = diff, let next = d.next else { return }
        loadingMore = true
        defer { loadingMore = false }
        do {
            let page = try await hub.diff(ref.sessionId, path: path, from: Int(next))
            var merged = d
            merged.hunks += page.hunks
            merged.next = page.next
            diff = merged
        } catch {
            self.error = error.localizedDescription
        }
    }
}

/// Asks the agent about the selected lines with an ordinary message.
struct AskSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(Voice.self) private var voice
    let sessionId: String
    let path: String
    let lines: [(Int, String)]
    let done: () -> Void
    /// The × takes the quoted lines out; then only the question goes.
    @State private var quoted = true
    @State private var question = ""
    @State private var sending = false
    @State private var note: String?
    @FocusState private var focused: Bool
    @State private var requestId = UUID().uuidString

    private var session: Schemas.Session? { model.sessions.first { $0.id == sessionId } }
    private var agent: String { session?.label ?? "the agent" }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                Button("Cancel", action: done).font(Face.geist(16, .medium)).foregroundStyle(Ink.ac)
                Spacer()
                Text("To \(agent)").font(Face.geist(13)).foregroundStyle(Ink.mut).lineLimit(1)
            }
            if quoted {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 8) {
                    Image(systemName: "doc").foregroundStyle(Ink.mut)
                    Text(DiffQuote.location(path: path, numbers: lines.map(\.0))).font(Face.mono(12.5)).foregroundStyle(Ink.ink)
                        .lineLimit(1).truncationMode(.head)
                    Spacer(minLength: 0)
                    Button { quoted = false } label: { Image(systemName: "xmark").foregroundStyle(Ink.mut).frame(width: 32, height: 32) }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Remove the quoted lines")
                }
                .padding(.leading, 12).padding(.trailing, 4).padding(.vertical, 6)
                Hairline()
                Text(lines.map(\.1).joined(separator: "\n"))
                    .font(Face.mono(12)).foregroundStyle(Ink.ink).lineLimit(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
            }
            .background(Ink.sf, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay { RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Ink.line) }
            }
            SectionHead("Your message")
            TextEditor(text: $question)
                .font(Face.geist(18, .medium))
                .focused($focused)
                .scrollContentBackground(.hidden)
                .padding(10)
                .frame(minHeight: 140)
                .background(Ink.sf, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay { RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Ink.line) }
                .accessibilityIdentifier("askQuestion")
            Note(text: note ?? (quoted
                    ? "Sent as an ordinary message. The agent gets the file, the line numbers and these lines, then answers in Conversation."
                    : "Sent as an ordinary message, without the lines."),
                 color: note == nil ? Ink.mut : Ink.bad)
            HStack(spacing: 12) {
                Button { focused = false; voice.dictate(after: question) } label: { MicDisc() }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Dictate the message")
                Button(sending ? "Sending…" : "Send to \(agent)") { Task { await send() } }
                    .buttonStyle(BigButtonStyle())
                    .disabled(sending || voice.dictating || question.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || session == nil)
                    .accessibilityIdentifier("askSend")
            }
        }
        .padding(20)
        .background(Ink.bg)
        .presentationDetents([.large])
        .overlay { if voice.dictating { VoiceOverlay() } }
        .interactiveDismissDisabled(voice.dictating)
        .onAppear { focused = true }
        // Dictation ends in review; its words come here, where they stay editable.
        .onChange(of: voice.phase) { _, phase in
            if phase == .review, voice.dictating, !voice.retrying, !voice.canRetry { takeDictation() }
        }
    }

    private func takeDictation() {
        let words = voice.text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !words.isEmpty { question = words }
        voice.finish()
    }

    private func send() async {
        guard let hub = model.hub, let session else { return }
        sending = true
        defer { sending = false }
        let text = quoted ? DiffQuote.message(path: path, lines: lines, question: question)
            : question.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            let r = try await hub.message(text, requestId: requestId, current: nil, target: .session(session))
            if r.outcome == "sent" || r.outcome == "started" {
                model.record(sent: text, to: r.current, started: false)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                done()
            } else {
                note = r.message
            }
        } catch {
            note = error.localizedDescription
        }
    }
}

extension Array {
    subscript(safe i: Int) -> Element? { indices.contains(i) ? self[i] : nil }
}
