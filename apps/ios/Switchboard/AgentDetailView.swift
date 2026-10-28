import SwiftUI
import SwitchboardKit

/// A Herdr agent Switchboard didn't start: its latest turns, refreshed while on screen. It has
/// no permission hooks, so its dialogs are answered in Herdr. The mic talks to it.
struct AgentDetailView: View {
    @Environment(AppModel.self) private var model
    @Environment(Voice.self) private var voice
    let agent: AgentRef
    @State private var convo: ConversationModel

    init(agent: AgentRef) {
        self.agent = agent
        _convo = State(initialValue: ConversationModel(.agent(machine: agent.machine, name: agent.name)))
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
                    Text(title).font(.headline)
                }
                Text("This agent asks for approval in Herdr, on its machine, not here.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                if let said = model.said[AppModel.saidKey(for: target)] { SaidBubble(said: said) }
                VStack(alignment: .leading, spacing: 10) {
                    Text("Latest").font(.footnote.weight(.semibold)).foregroundStyle(.secondary).textCase(.uppercase)
                    if let why = convo.unavailable {
                        Label("No conversation to show (\(why)).", systemImage: "info.circle")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    } else {
                        ConversationPreview(convo: convo)
                    }
                    NavigationLink(value: ConversationRef(source: .agent(machine: agent.machine, name: agent.name), title: label)) {
                        Label(convo.unavailable == nil ? "Open the conversation" : "See the agent's screen", systemImage: "chevron.right")
                            .labelStyle(TrailingIcon())
                    }
                }
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
        .task(id: model.wakes) { await convo.follow(model) }
    }
}
