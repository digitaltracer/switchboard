import SwiftUI
import SwitchboardKit

extension Color {
    /// Behind ended sessions and agents, which sit at the bottom of their lists.
    static let endedRow = Color(.tertiarySystemGroupedBackground)
}

extension HerdrSession {
    /// "3 agents · 1 working", or when it ended.
    var summary: String {
        if isEnded { return "Ended \(Ago.short(endedAt)) ago".replacingOccurrences(of: "now ago", with: "just now") }
        var parts = ["\(live.count) \(live.count == 1 ? "agent" : "agents")"]
        let needs = live.filter(\.needsYou).count
        let working = live.filter { $0.agentStatus == "working" }.count
        if needs > 0 { parts.append("\(needs) need\(needs == 1 ? "s" : "") you") }
        if working > 0 { parts.append("\(working) working") }
        if needs == 0 && working == 0 { parts.append(live.count == 1 ? "idle" : "all idle") }
        return parts.joined(separator: " · ")
    }

    var status: SessionStatus {
        if isEnded { return .ended }
        if live.contains(where: \.needsYou) { return SessionStatus(agentStatus: "blocked") }
        if live.contains(where: { $0.agentStatus == "working" }) { return SessionStatus(agentStatus: "working") }
        return SessionStatus(agentStatus: "idle")
    }
}

extension HerdrSession.Agent {
    var status: SessionStatus {
        switch self {
        case .task(let s): SessionStatus(s)
        case .live(let a): SessionStatus(agentStatus: a.agentStatus)
        case .ended: .ended
        }
    }

    var isEnded: Bool { agentStatus == "ended" }

    /// "Working · Claude", "Needs you · fix-flaky", or "Ended 2h ago".
    var subtitle: String {
        switch self {
        case .task(let s):
            let when = isEnded ? "Ended \(Ago.short(s.updatedAt)) ago" : status.text
            return [when, s.agent.capitalized, s.branch].compactMap { $0 }.joined(separator: " · ")
        case .live(let a):
            return [status.text, a.kind?.capitalized].compactMap { $0 }.joined(separator: " · ")
        case .ended(let e):
            return ["Ended \(Ago.short(e.endedAt)) ago", e.kind?.capitalized].compactMap { $0 }.joined(separator: " · ")
        }
    }

    /// Where tapping it goes; an agent that ended outside Switchboard has nothing more to show.
    var destination: AnyHashable? {
        switch self {
        case .task(let s): s.id
        case .live(let a): AgentRef(machine: a.machineId, name: a.name)
        case .ended: nil
        }
    }
}

/// One Herdr session, as a row of the sessions list.
struct SessionRow: View {
    let session: HerdrSession
    let showMachine: Bool
    let first: Bool

    var body: some View {
        let status = session.status
        NavigationLink(value: session.key) {
            ListRow(symbol: session.isEnded ? "moon.zzz" : "rectangle.stack", tint: session.isEnded ? .secondary : status.color,
                    title: session.key.name,
                    subtitle: showMachine ? "\(session.summary) · \(session.key.machine)" : session.summary,
                    trailing: "", first: first)
        }
        .background(session.isEnded ? Color.endedRow : .clear)
        .accessibilityIdentifier("session-\(session.key.name)")
    }
}

/// A Herdr session's agents: running ones first, then those that ended today.
struct HerdrSessionView: View {
    @Environment(AppModel.self) private var model
    let key: HerdrSession.Key

    private var session: HerdrSession {
        model.herdrSessions(machine: key.machine, since: HerdrSession.startOfToday()).first { $0.key == key }
            ?? HerdrSession(key: key)
    }

    var body: some View {
        let session = session
        List {
            Section {
                ForEach(session.live, id: \.self) { row($0) }
                ForEach(session.ended, id: \.self) { row($0).listRowBackground(Color.endedRow) }
                if session.agents.isEmpty { Text("No agents in this session.").foregroundStyle(.secondary) }
            } header: {
                Text("Agents")
            } footer: {
                Text("On \(key.machine). `herdr session attach \(key.name)` opens it there.")
            }
        }
        .navigationTitle(key.name)
        .task(id: model.wakes) {
            while !Task.isCancelled {
                await model.refreshAgents()
                try? await Task.sleep(for: .seconds(6))
            }
        }
        .refreshable { await model.refreshAgents() }
    }

    @ViewBuilder
    private func row(_ a: HerdrSession.Agent) -> some View {
        let label = AgentLabel(agent: a)
        if let to = a.destination {
            NavigationLink(value: to) { label }
        } else {
            label
        }
    }
}

private struct AgentLabel: View {
    let agent: HerdrSession.Agent

    var body: some View {
        let status = agent.status
        HStack(spacing: 14) {
            Image(systemName: status.symbol)
                .font(.system(size: 19))
                .foregroundStyle(status.color)
                .frame(width: 26)
            VStack(alignment: .leading, spacing: 2) {
                Text(agent.shortName).font(.headline).lineLimit(1)
                    .foregroundStyle(agent.isEnded ? .secondary : .primary)
                Text(agent.subtitle).font(.subheadline).foregroundStyle(agent.isEnded ? .secondary : status.color).lineLimit(1)
            }
        }
        .padding(.vertical, 2)
    }
}
