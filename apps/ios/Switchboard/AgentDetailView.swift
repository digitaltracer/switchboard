import SwiftUI
import SwitchboardKit

/// A Herdr agent Switchboard didn't start: its latest turns, refreshed while on screen. A dialog
/// on its screen is answered from its card when Switchboard recognises it. The mic talks to it.
struct AgentDetailView: View {
    @Environment(AppModel.self) private var model
    @Environment(Voice.self) private var voice
    let agent: AgentRef
    @State private var convo: ConversationModel

    init(agent: AgentRef) {
        self.agent = agent
        _convo = State(initialValue: ConversationModel(.agent(machine: agent.machine, name: agent.name), drawsTerminal: false))
    }

    private var target: Schemas.TargetView { .agent(machine: agent.machine, name: agent.name, label: label) }
    private var info: Schemas.AgentView? {
        model.agents.first { $0.machineId == agent.machine && $0.name == agent.name }
    }
    private var label: String { info?.displayName ?? agent.name }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                HStack(spacing: 8) {
                    if let info { StatusPill(status: SessionStatus(agentStatus: info.agentStatus)) }
                    Text([info?.kind?.capitalized, agent.machine].compactMap { $0 }.joined(separator: " · "))
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                .padding(.top, -6)
                if let title = info?.title, !title.isEmpty {
                    Text(title).font(.subheadline.weight(.semibold))
                }
                DialogCard(machine: agent.machine, agent: agent.name, label: label)
                if let said = model.said[AppModel.saidKey(for: target)] { SaidBubble(said: said) }
                SummaryCard(convo: convo, conversation: ConversationRef(source: .agent(machine: agent.machine, name: agent.name), title: label))
                ConversationLinks(source: .agent(machine: agent.machine, name: agent.name), title: label,
                                  hasConversation: convo.unavailable == nil)
            }
            .padding(.horizontal, 20)
            .padding(.bottom, 120)
        }
        .safeAreaInset(edge: .bottom) {
            HStack {
                Color.clear.frame(maxWidth: .infinity, maxHeight: 1)
                MicButton(size: 60) { voice.listen(to: target) }
                    .accessibilityLabel("Talk to \(label)")
                Button { voice.type(to: target) } label: { Image(systemName: "keyboard") }
                    .font(.system(size: 21))
                    .accessibilityLabel("Type a message to \(label)")
                    .frame(maxWidth: .infinity)
            }
            .padding(.horizontal, 8)
            .padding(.top, 6)
            .background(.bar)
        }
        .background(Color(.systemGroupedBackground))
        .navigationTitle(label)
        .navigationBarTitleDisplayMode(.large)
        .refreshable { if let hub = model.hub { await convo.refresh(hub) } }
        .task(id: model.followKey) { await convo.follow(model) }
        // Its status pill: only home refreshes agents otherwise, and home isn't running now.
        .task(id: model.followKey) {
            while model.followKey != nil, !Task.isCancelled {
                await model.refreshAgents()
                try? await Task.sleep(for: .seconds(5))
            }
        }
    }
}
