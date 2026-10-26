import SwiftUI
import SwitchboardKit
import UIKit

extension Color {
    /// Text tints that keep 4.5:1 on light backgrounds and use the system colour in dark mode.
    static let needs = adaptive(light: UIColor(red: 0.70, green: 0.31, blue: 0, alpha: 1), dark: .systemOrange)
    static let working = adaptive(light: UIColor(red: 0, green: 0.35, blue: 0.69, alpha: 1), dark: .systemBlue)
    static let done = adaptive(light: UIColor(red: 0.12, green: 0.49, blue: 0.20, alpha: 1), dark: .systemGreen)
    static let codeBackground = Color(red: 0.11, green: 0.11, blue: 0.12)

    private static func adaptive(light: UIColor, dark: UIColor) -> Color {
        Color(UIColor { $0.userInterfaceStyle == .dark ? dark : light })
    }
}

/// How a session's state reads at a glance.
struct SessionStatus {
    let text: String
    let color: Color
    let symbol: String

    init(_ s: Schemas.Session) {
        if s.hookFailed {
            self.init("Stuck", .red, "exclamationmark.triangle")
        } else if !s.pendingApprovals.isEmpty {
            self.init("Needs you", .needs, "checkmark.shield")
        } else if s.lifecycle == "starting" {
            self.init("Starting", .secondary, "hourglass")
        } else if s.lifecycle != "running" {
            self.init(s.lifecycle.capitalized, s.lifecycle == "failed" ? .red : .secondary, "stop.circle")
        } else if s.machineStatus != "ready" {
            self.init("\(s.machineId) \(s.machineStatus)", .secondary, "wifi.slash")
        } else {
            self.init(agentStatus: s.agentStatus)
        }
    }

    static let ended = SessionStatus("Ended", .secondary, "moon.zzz")

    init(agentStatus: String) {
        switch agentStatus {
        case "working": self.init("Working", .working, "waveform.path.ecg")
        case "blocked": self.init("Blocked", .needs, "pause.circle")
        case "idle": self.init("Idle", .done, "checkmark.circle")
        case "done": self.init("Done", .done, "checkmark.circle")
        default: self.init(agentStatus.capitalized, .secondary, "circle.dashed")
        }
    }

    private init(_ text: String, _ color: Color, _ symbol: String) {
        self.text = text
        self.color = color
        self.symbol = symbol
    }
}

struct StatusPill: View {
    let status: SessionStatus

    var body: some View {
        Text(status.text)
            .font(.footnote.weight(.semibold))
            .padding(.horizontal, 9)
            .frame(height: 24)
            .background(status.color.opacity(0.14), in: Capsule())
            .foregroundStyle(status.color)
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
}

/// A pending approval as a card, with its countdown. A permission can be approved with Face ID
/// or denied; a question or plan can only be denied here, since approving would leave the
/// agent at a dialog nobody can answer from the phone.
struct ApprovalCard: View {
    @Environment(AppModel.self) private var model
    let approval: Schemas.Approval
    let session: Schemas.Session
    /// Shows the session's name as a link to it, on the home screen.
    var open: (() -> Void)?
    let decided: () async -> Void
    @State private var busy = false
    @State private var result: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Button { open?() } label: {
                HStack(spacing: 12) {
                    Image(systemName: approval.kind == "permission" ? "checkmark.shield" : "questionmark.bubble")
                        .font(.system(size: 19, weight: .semibold))
                        .frame(width: 40, height: 40)
                        .background(Color.needs.opacity(0.14), in: RoundedRectangle(cornerRadius: 11, style: .continuous))
                        .foregroundStyle(Color.needs)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(title).font(.headline).foregroundStyle(.primary).multilineTextAlignment(.leading)
                        TimelineView(.periodic(from: .now, by: 1)) { _ in
                            Text("Denied automatically in \(Countdown.text(deadline: approval.deadline, now: Countdown.nowMs()))")
                                .font(.footnote)
                                .monospacedDigit()
                                .foregroundStyle(Color.needs)
                        }
                    }
                    Spacer(minLength: 0)
                    if open != nil { Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary) }
                }
            }
            .buttonStyle(.plain)
            .disabled(open == nil)
            switch approval.kind {
            case "question":
                ForEach(approval.questions, id: \.self) { Text($0).font(.body) }
                Text("Answer questions in Herdr for now.").font(.footnote).foregroundStyle(.secondary)
            case "plan":
                Text(approval.plan ?? "").font(.callout)
                Text("Approve plans in Herdr for now.").font(.footnote).foregroundStyle(.secondary)
            default:
                Text(approval.inputText)
                    .font(.system(.callout, design: .monospaced))
                    .foregroundStyle(.white)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 12)
                    .background(Color.codeBackground, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .textSelection(.enabled)
            }
            if let result { Text(result).font(.footnote).foregroundStyle(.secondary) }
            HStack(spacing: 10) {
                Button { Task { await deny() } } label: {
                    Text("Deny").frame(maxWidth: .infinity, minHeight: 50)
                        .background(Color(.tertiarySystemFill), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                        .foregroundStyle(.primary)
                }
                .accessibilityIdentifier("deny-\(approval.id)")
                if approval.kind == "permission" {
                    Button { Task { await approve() } } label: {
                        Label("Approve", systemImage: "faceid").frame(maxWidth: .infinity, minHeight: 50)
                            .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                            .foregroundStyle(.white)
                    }
                    .accessibilityIdentifier("approve-\(approval.id)")
                }
            }
            .font(.headline)
            .buttonStyle(.plain)
            .disabled(busy)
        }
        .padding(16)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 20, style: .continuous))
    }

    private var title: String {
        switch approval.kind {
        case "question": return "\(session.label) has a question"
        case "plan": return "\(session.label) has a plan for you"
        default: return open == nil ? "Run this?" : "\(session.label) wants to run"
        }
    }

    private func approve() async {
        guard let hub = model.hub else { return }
        busy = true
        defer { busy = false }
        do {
            let r = try await hub.approve(approval.id, reason: "Approve \(approval.toolName) in \(session.label)", requestId: UUID().uuidString)
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

/// What this phone last said there, kept on the phone.
struct SaidBubble: View {
    let said: AppModel.Said

    var body: some View {
        VStack(alignment: .trailing, spacing: 4) {
            Text(said.text)
                .padding(.horizontal, 13)
                .padding(.vertical, 9)
                .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                .foregroundStyle(.white)
            Text("You said · \(said.at.formatted(.relative(presentation: .named)))")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
    }
}

/// Changed files, laid out like attachments.
struct ChangesGrid: View {
    let changes: Schemas.Changes

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(changes.files.count == 1 ? "1 changed file" : "\(changes.files.count)\(changes.truncated ? "+" : "") changed files")
                .font(.footnote.weight(.semibold))
                .foregroundStyle(.secondary)
                .textCase(.uppercase)
            LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible())], spacing: 10) {
                ForEach(changes.files, id: \.path) { f in
                    HStack(spacing: 10) {
                        Image(systemName: "doc.text")
                            .font(.system(size: 22))
                            .foregroundStyle(.secondary)
                        VStack(alignment: .leading, spacing: 1) {
                            Text((f.path as NSString).lastPathComponent)
                                .font(.subheadline.weight(.semibold))
                                .lineLimit(1)
                            if f.added == nil && f.deleted == nil {
                                Text("new").font(.footnote).foregroundStyle(Color.done)
                            } else {
                                HStack(spacing: 4) {
                                    Text("+\(f.added ?? 0)").foregroundStyle(Color.done)
                                    Text("−\(f.deleted ?? 0)").foregroundStyle(.red)
                                }
                                .font(.footnote.monospacedDigit())
                            }
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(12)
                    .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel(f.path)
                    .accessibilityIdentifier(f.path)
                }
            }
        }
    }
}
