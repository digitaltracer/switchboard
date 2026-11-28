import SwiftUI
import SwitchboardKit

/// The Inbox card "While you were away": each agent that did something since the phone last had
/// the app open, needs-you first, until "Mark all read". An opt-in summary replaces the rows.
struct AwayCard: View {
    @Environment(AppModel.self) private var model
    let activity: Schemas.Activity

    private var span: String { AwayDigest.duration(TimeInterval(activity.now - activity.since) / 1000) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                Text("While you were away").font(Face.geist(17, .semibold)).foregroundStyle(Ink.ink)
                Spacer()
                Button("Mark all read") { withAnimation(.snappy) { model.markAwaySeen() } }
                    .font(Face.geist(14, .medium)).foregroundStyle(Ink.ac)
                    .accessibilityIdentifier("markAwaySeen")
            }
            .padding(16)
            if let summary = model.awaySummary {
                Text(summary).font(Face.geist(15)).foregroundStyle(Ink.ink)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 16).padding(.bottom, 14)
                    .accessibilityIdentifier("awaySummary")
            } else {
                ForEach(activity.agents, id: \.agent) { a in
                    NavigationLink(value: link(a)) { row(a) }.buttonStyle(.plain)
                }
            }
            Hairline()
            Text("\(model.awaySummary == nil ? "Since you last opened the app" : "Summarised by Claude, from the hub's facts") · \(span)")
                .font(Face.meta).foregroundStyle(Ink.mut)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(16)
        }
        .background(Ink.sf, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay { RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(Ink.line) }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("awayCard")
    }

    private func link(_ a: Schemas.AwayAgentView) -> AgentPageRef {
        if let s = a.sessionId { return AgentPageRef(source: .session(s)) }
        return AgentPageRef(source: .agent(machine: a.machineId, name: a.agent))
    }

    private func title(_ a: Schemas.AwayAgentView) -> String {
        if let s = model.sessions.first(where: { $0.id == a.sessionId }) { return s.label }
        if let live = model.agents.first(where: { $0.machineId == a.machineId && $0.name == a.agent }) { return live.displayName }
        return a.agent
    }

    private func row(_ a: Schemas.AwayAgentView) -> some View {
        let tone = AwayDigest.tone(a)
        let colour: Color = switch tone {
        case "need": Ink.warn
        case "bad": Ink.bad
        case "working": Ink.ac
        default: Ink.ok
        }
        return HStack(alignment: .top, spacing: 12) {
            Circle().fill(colour).frame(width: 8, height: 8).padding(.top, 6)
            VStack(alignment: .leading, spacing: 2) {
                Text(title(a)).font(Face.geist(15, .semibold)).foregroundStyle(Ink.ink).lineLimit(1)
                Text(AwayDigest.line(a, now: activity.now)).font(Face.geist(13.5)).foregroundStyle(Ink.sub).lineLimit(2)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 16).padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(tone == "need" ? Ink.wbg : .clear)
        .overlay(alignment: .top) { Hairline() }
        .contentShape(Rectangle())
    }
}

/// An agent page's "Since you looked": the facts in order, and its last reply when the page doesn't show it.
struct AwayTimeline: View {
    let agent: Schemas.AwayAgentView
    let since: Int64
    let now: Int64
    /// The agent's last reply, quoted under its last turn; nil when the page shows it already.
    var reply: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            SectionHead("Since you looked · \(AwayDigest.duration(TimeInterval(now - since) / 1000))")
            VStack(alignment: .leading, spacing: 0) {
                let rows = AwayDigest.timeline(agent.facts)
                ForEach(Array(rows.enumerated()), id: \.offset) { i, f in
                    HStack(alignment: .firstTextBaseline, spacing: 14) {
                        Text(Date(timeIntervalSince1970: TimeInterval(f.at) / 1000).formatted(date: .omitted, time: .shortened))
                            .font(Face.mono(12)).foregroundStyle(Ink.mut).frame(width: 64, alignment: .leading)
                        VStack(alignment: .leading, spacing: 4) {
                            Text(f.text).font(Face.geist(15, f.kind == "finished" ? .medium : .regular))
                                .foregroundStyle(f.kind == "finished" ? Ink.ok : f.kind == "error" ? Ink.bad : Ink.ink)
                            if f.kind == "finished", i == rows.lastIndex(where: { $0.kind == "finished" }), let r = reply {
                                Text((try? AttributedString(markdown: "\"\(r)\"")) ?? AttributedString("\"\(r)\""))
                                    .font(Face.geist(15)).foregroundStyle(Ink.sub)
                            }
                        }
                        .fixedSize(horizontal: false, vertical: true)
                    }
                    .padding(.vertical, 11)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .overlay(alignment: .top) { if i > 0 { Hairline() } }
                }
            }
            .padding(.horizontal, 16)
            .background(Ink.sf, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .overlay { RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Ink.line) }
            Note(text: reply == nil ? "Facts the hub recorded. No summarising model." : "Facts the hub recorded, plus the agent's own last reply. No summarising model.").padding(.top, 8)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("sinceYouLooked")
    }
}

/// Settings › While you were away: the opt-in summary and its model, kept on the hub.
struct AwaySettingsSection: View {
    @Environment(AppModel.self) private var model
    @State private var settings: Schemas.AwaySettingsView?
    @State private var model_ = ""
    @State private var error: String?

    var body: some View {
        SectionHead("While you were away")
        VStack(alignment: .leading, spacing: 10) {
            Toggle(isOn: Binding(get: { settings?.summary ?? false }, set: { on in save(summary: on) })) {
                Text("Summarise with Claude").font(Face.geist(16)).foregroundStyle(Ink.ink)
            }
            .tint(Ink.ink)
            .disabled(settings == nil)
            .accessibilityIdentifier("awaySummaryToggle")
            Note(text: "Off: you see the facts your hub recorded. On: when this phone comes back, your machine runs claude -p on your own Claude account, with every tool, hook, plugin and MCP server off, to summarise them. Masked first, like everything that leaves the machine.")
            if settings?.summary == true {
                HStack {
                    Text("Model").font(Face.geist(15)).foregroundStyle(Ink.sub)
                    TextField("haiku", text: $model_)
                        .font(Face.mono(14)).multilineTextAlignment(.trailing)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        .onSubmit { save(summary: true) }
                }
                .frame(minHeight: 44)
            }
            if let error { Note(text: error, color: Ink.bad) }
        }
        .task { await load() }
    }

    private func load() async {
        guard let hub = model.hub, let s = try? await hub.awaySettings() else { return }
        settings = s
        model_ = s.model
    }

    private func save(summary: Bool) {
        let next = Schemas.AwaySettingsView(summary: summary, model: model_.isEmpty ? "haiku" : model_)
        Task {
            guard let hub = model.hub else { return }
            do {
                try await hub.setAwaySettings(next)
                settings = next
                error = nil
            } catch {
                self.error = error.localizedDescription
            }
        }
    }
}
