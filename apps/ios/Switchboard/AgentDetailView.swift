import SwiftUI
import SwitchboardKit

/// A named Herdr agent Switchboard didn't start: its output, refreshed while on screen. It has
/// no permission hooks, so its dialogs are answered in Herdr. The mic talks to it.
struct AgentDetailView: View {
    @Environment(AppModel.self) private var model
    @Environment(Voice.self) private var voice
    let agent: AgentRef
    @State private var output = ""
    @State private var error: String?
    @State private var showingOutput = false

    private var target: Schemas.TargetView { .agent(machine: agent.machine, name: agent.name) }
    private var info: Schemas.AgentView? {
        model.agents.first { $0.machineId == agent.machine && $0.name == agent.name }
    }

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
                Text("Started outside Switchboard, so it has no approvals here. Answer its dialogs in Herdr.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                if let error { Text(error).font(.footnote).foregroundStyle(.red) }
                if let said = model.said[AppModel.saidKey(for: target)] { SaidBubble(said: said) }
                VStack(alignment: .leading, spacing: 8) {
                    Text("Latest").font(.footnote.weight(.semibold)).foregroundStyle(.secondary).textCase(.uppercase)
                    OutputBlock(text: output)
                        .accessibilityIdentifier("agentOutput")
                    Button { showingOutput = true } label: {
                        Label("Show the full output", systemImage: "chevron.right").labelStyle(TrailingIcon())
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
                    .accessibilityLabel("Talk to \(agent.name)")
                Button { voice.type(to: target) } label: { Image(systemName: "keyboard") }
                    .font(.system(size: 21))
                    .accessibilityLabel("Type a message to \(agent.name)")
                    .frame(maxWidth: .infinity)
            }
            .padding(.horizontal, 8)
            .padding(.top, 6)
            .background(.bar)
        }
        .background(Color(.systemGroupedBackground))
        .navigationTitle(agent.name)
        .navigationBarTitleDisplayMode(.large)
        .refreshable { await load() }
        .sheet(isPresented: $showingOutput) { FullOutput(title: agent.name, text: output) }
        .task(id: model.wakes) {
            while !Task.isCancelled {
                await load()
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    private func load() async {
        guard let hub = model.hub else { return }
        do {
            output = try await hub.agentOutput(machine: agent.machine, agent: agent.name, lines: 300).text
            error = nil
        } catch {
            if !Task.isCancelled { self.error = error.localizedDescription }
        }
    }
}
