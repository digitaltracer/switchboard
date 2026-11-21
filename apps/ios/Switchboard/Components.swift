import SwiftUI
import SwitchboardKit
import UIKit

extension Color {
    static let needs = Ink.warn
    static let working = Ink.ac
    static let done = Ink.ok
    static let codeBackground = Ink.code
    /// An agent's reply.
    static let replyText = Ink.ink
    /// Nested list items, one step back from a reply's text.
    static let replyDim = Ink.sub
    /// Behind inline code.
    static let codeFill = Ink.fill
}

/// How a session's or agent's state reads at a glance: its word and its glyph.
struct SessionStatus {
    let text: String
    let glyph: Glyph

    init(_ s: Schemas.Session) {
        if s.hookFailed {
            self.init("Stuck", .need)
        } else if !s.pendingApprovals.isEmpty {
            self.init("Needs you", .need)
        } else if s.lifecycle == "starting" {
            self.init("Starting", .work)
        } else if s.lifecycle != "running" {
            self.init(s.lifecycle.capitalized, .end)
        } else if s.machineStatus != "ready" {
            self.init("\(s.machineId) \(s.machineStatus)", .idle)
        } else {
            self.init(agentStatus: s.agentStatus)
        }
    }

    static let ended = SessionStatus("Ended", .end)
    /// A message just went, and the agent hasn't been seen to pick it up yet.
    static let sent = SessionStatus("Sent", .work)

    init(agentStatus: String) {
        switch agentStatus {
        case "working": self.init("Working", .work)
        case "blocked": self.init("Needs you", .need)
        case "idle": self.init("Idle", .idle)
        case "done": self.init("Done", .done)
        case "ended": self.init("Ended", .end)
        default: self.init(agentStatus.capitalized, .idle)
        }
    }

    private init(_ text: String, _ glyph: Glyph) {
        self.text = text
        self.glyph = glyph
    }

    var color: Color {
        switch glyph {
        case .need: Ink.warn
        case .work: Ink.ac
        case .done: Ink.ok
        case .idle, .end: Ink.sub
        }
    }
}

enum Ago {
    /// `now`, `5m`, `2h` or `3d` since a hub timestamp in milliseconds.
    static func short(_ ms: Int64, now: Date = Date()) -> String {
        let s = max(0, Int(now.timeIntervalSince1970) - Int(ms / 1000))
        switch s {
        case ..<60: return "now"
        case ..<3600: return "\(s / 60)m"
        case ..<86400: return "\(s / 3600)h"
        default: return "\(s / 86400)d"
        }
    }

    /// "just now", "12m ago", "6 days ago".
    static func long(_ ms: Int64, now: Date = Date()) -> String {
        let s = max(0, Int(now.timeIntervalSince1970) - Int(ms / 1000))
        switch s {
        case ..<60: return "just now"
        case ..<3600: return "\(s / 60)m ago"
        case ..<86400: return "\(s / 3600)h ago"
        case ..<172_800: return "yesterday"
        default: return "\(s / 86400) days ago"
        }
    }
}

/// A pending approval as a card, with its countdown. A permission or plan is approved with Face
/// ID or denied; a question is answered by picking its options, then sending them with Face ID.
/// Unanswered at zero, the hub denies it.
struct ApprovalCard: View {
    @Environment(AppModel.self) private var model
    let approval: Schemas.Approval
    let session: Schemas.Session
    /// Shows the session's name as a link to it, on the home screen.
    var open: (() -> Void)?
    let decided: () async -> Void
    @State private var busy = false
    @State private var result: String?
    @State private var picked: [Int: Set<String>] = [:]

    var body: some View {
        CardBox {
            HStack(spacing: 10) {
                StatusGlyph(glyph: .need)
                Button { open?() } label: {
                    Text(title)
                        .font(Face.title)
                        .foregroundStyle(Ink.ink)
                        .lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(.plain)
                .disabled(open == nil)
            }
            switch approval.kind {
            case "question":
                ForEach(Array(approval.questionOptions.enumerated()), id: \.offset) { i, q in
                    VStack(alignment: .leading, spacing: 0) {
                        Text(q.question).font(Face.say).foregroundStyle(Ink.ink).fixedSize(horizontal: false, vertical: true)
                        if q.multiSelect { Note(text: "Pick any that apply.").padding(.top, 4) }
                        OptionList(
                            options: q.options.map { OptionItem(id: $0.id, label: $0.label, detail: $0.description) },
                            selected: picked[i] ?? [],
                            multi: q.multiSelect,
                            disabled: busy
                        ) { o in pick(o.id, in: i, multi: q.multiSelect) }
                    }
                }
            case "plan":
                ScrollView {
                    MarkdownText(text: approval.plan ?? "").frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 260)
            default:
                CodeBlock(text: approval.inputText, wraps: approval.inputText.count > 60)
            }
            TimelineView(.periodic(from: .now, by: 1)) { _ in
                VStack(alignment: .leading, spacing: 8) {
                    CountdownBar(created: approval.createdAt, deadline: approval.deadline)
                    Text("Denies itself in \(Countdown.text(deadline: approval.deadline, now: Countdown.nowMs()))")
                        .font(Face.mono(13, .medium))
                        .foregroundStyle(Ink.warn)
                        .monospacedDigit()
                }
            }
            if let result { Note(text: result, color: Ink.sub) }
            HStack(spacing: 10) {
                Button { Task { await deny() } } label: { Text("Deny") }
                    .buttonStyle(BigButtonStyle(kind: .outline))
                    .frame(maxWidth: .infinity)
                    .accessibilityIdentifier("deny-\(approval.id)")
                if approval.kind != "question" || answered {
                    Button { Task { await approve() } } label: {
                        Label(approval.kind == "question" ? "Send" : "Approve", systemImage: "faceid")
                    }
                    .buttonStyle(BigButtonStyle())
                    .frame(maxWidth: .infinity)
                    .accessibilityIdentifier("approve-\(approval.id)")
                }
            }
            .disabled(busy)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("approval-\(approval.id)")
    }

    private var title: String {
        switch approval.kind {
        case "question": return open == nil ? "It has a question" : "\(session.label) has a question"
        case "plan": return open == nil ? "Its plan, for you to approve" : "\(session.label) has a plan for you"
        default: return open == nil ? "Run this?" : "\(session.label) wants to run"
        }
    }

    /// Every question has its answer.
    private var answered: Bool {
        approval.questionOptions.indices.allSatisfy { !(picked[$0] ?? []).isEmpty }
    }

    private func pick(_ id: String, in question: Int, multi: Bool) {
        var s = picked[question] ?? []
        if multi { if s.contains(id) { s.remove(id) } else { s.insert(id) } } else { s = [id] }
        picked[question] = s
    }

    private func approve() async {
        guard let hub = model.hub else { return }
        busy = true
        defer { busy = false }
        do {
            let reason = "Approve \(approval.toolName) in \(session.label)"
            let r = approval.kind == "permission"
                ? try await hub.approve(approval.id, reason: reason, requestId: UUID().uuidString)
                : try await hub.answer(approval, choices: picked.keys.sorted().flatMap { (picked[$0] ?? []).sorted() },
                                       reason: reason, requestId: UUID().uuidString)
            result = describe(r)
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        } catch {
            result = error.localizedDescription
        }
        await decided()
    }

    private func deny() async {
        guard let hub = model.hub else { return }
        busy = true
        defer { busy = false }
        do {
            result = describe(try await hub.deny(approval.id, requestId: UUID().uuidString))
        } catch {
            result = error.localizedDescription
        }
        await decided()
    }

    private func describe(_ r: Schemas.DecisionResponse) -> String {
        if r.replayed { return "Already decided: \(r.outcome)." }
        if r.outcome == "allow" { return r.delivery == "unknown" ? "Approved; the hub lost the connection, so it may have run." : "Approved." }
        return "Denied."
    }
}

/// The time left on an approval, as a thin bar that empties.
struct CountdownBar: View {
    let created: Int64
    let deadline: Int64

    var body: some View {
        let total = max(1, deadline - created)
        let left = min(1, max(0, Double(deadline - Countdown.nowMs()) / Double(total)))
        GeometryReader { g in
            ZStack(alignment: .leading) {
                Capsule().fill(Ink.fill)
                Capsule().fill(Ink.warn).frame(width: g.size.width * left)
            }
        }
        .frame(height: 3)
        .accessibilityHidden(true)
    }
}

/// What this phone last said there, kept on the phone.
struct SaidBubble: View {
    let said: AppModel.Said

    var body: some View {
        VStack(alignment: .trailing, spacing: 4) {
            MeBubble(text: said.text)
            Text("You said · \(said.at.formatted(.relative(presentation: .named)))")
                .font(Face.meta)
                .foregroundStyle(Ink.mut)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
    }
}

/// The owner's own words, in a dark bubble on the right.
struct MeBubble: View {
    let text: String

    var body: some View {
        Text(text)
            .font(Face.geist(15.5, relativeTo: .body))
            .foregroundStyle(Ink.inv)
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .background(Ink.ink, in: UnevenRoundedRectangle(
                topLeadingRadius: 18, bottomLeadingRadius: 18, bottomTrailingRadius: 4, topTrailingRadius: 18, style: .continuous))
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.leading, 48)
            .textSelection(.enabled)
    }
}

/// Changed files as rows: the path, then what changed. For a session, each opens its diff.
struct ChangesList: View {
    let changes: Schemas.Changes
    var sessionId: String?

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(changes.files.enumerated()), id: \.element.path) { i, f in
                if let sessionId {
                    NavigationLink(value: DiffRef(sessionId: sessionId, paths: changes.files.map(\.path), index: i)) {
                        row(i, f, chevron: true)
                    }
                    .buttonStyle(.plain)
                } else {
                    row(i, f, chevron: false)
                }
            }
        }
    }

    private func row(_ i: Int, _ f: Schemas.ChangedFile, chevron: Bool) -> some View {
        HStack(spacing: 12) {
            Text(f.path)
                .font(Face.mono(12.5))
                .foregroundStyle(Ink.ink)
                .lineLimit(1)
                .truncationMode(.head)
                .frame(maxWidth: .infinity, alignment: .leading)
            if f.binary == true {
                Text("binary\(f.size.map { " · \(fileSize($0))" } ?? "")").font(Face.mono(12.5)).foregroundStyle(Ink.mut)
            } else if f.added == nil && f.deleted == nil {
                Text("new").font(Face.mono(12.5)).foregroundStyle(Ink.ok)
            } else {
                HStack(spacing: 4) {
                    Text("+\(f.added ?? 0)").foregroundStyle(Ink.ok)
                    Text("−\(f.deleted ?? 0)").foregroundStyle(Ink.bad)
                }
                .font(Face.mono(12.5))
            }
            if chevron { Chevron() }
        }
        .padding(.vertical, 8)
        .frame(minHeight: 44)
        .contentShape(Rectangle())
        .overlay(alignment: .top) { if i > 0 { Hairline() } }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(f.path)
        .accessibilityIdentifier(f.path)
    }

    var title: String {
        changes.files.count == 1 ? "1 changed file" : "\(changes.files.count)\(changes.truncated ? "+" : "") changed files"
    }
}
