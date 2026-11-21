import SwiftUI
import SwitchboardKit

/// Settings › Ship actions: which of the owner's skills show as buttons on a finished agent, for
/// all projects or one. Kept on the hub, so every paired phone shows the same buttons.
struct ShipSettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    /// `*` for all projects, else a project id.
    @State private var scope = "*"
    @State private var list: [ShipButton] = []
    @State private var skills: [Schemas.Skill] = []
    @State private var error: String?

    private var projects: [String] { model.projects.map(\.id) }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Button { dismiss() } label: { Label("Settings", systemImage: "chevron.left") }
                    .font(Face.geist(16, .medium)).foregroundStyle(Ink.ac).frame(minHeight: 44)
                Spacer()
            }
            .padding(.horizontal, 16)
            List {
                Section {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Ship actions").font(Face.h1).tracking(-0.84).foregroundStyle(Ink.ink)
                        Text("Buttons on a finished session. Each one asks the agent to run one of its own skills.")
                            .font(Face.geist(15)).foregroundStyle(Ink.sub)
                        scopePicker.padding(.top, 8)
                        if let error { Note(text: error, color: Ink.bad) }
                    }
                    .listRowBackground(Color.clear)
                    .listRowSeparator(.hidden)
                }
                Section {
                    if list.isEmpty {
                        Text(scope == "*" ? "None yet. Until you add some, a finished agent suggests your usual ship skills."
                                          : "None of its own: \(scope) shows the buttons for all projects.")
                            .font(Face.meta).foregroundStyle(Ink.mut)
                    }
                    ForEach(Array(list.enumerated()), id: \.element.id) { i, b in
                        HStack(spacing: 10) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(b.label).font(Face.geist(16, .medium)).foregroundStyle(Ink.ink)
                                Text("\(b.skill)\(source(b.skill).map { " · \($0)" } ?? "")\(b.afterPR ? " · after a PR" : "")")
                                    .font(Face.mono(12)).foregroundStyle(Ink.mut)
                            }
                            Spacer()
                            if i == list.firstIndex(where: { !$0.afterPR }) {
                                Text("Main").font(Face.geist(11, .semibold)).padding(.horizontal, 7).padding(.vertical, 3)
                                    .background(Ink.fill, in: RoundedRectangle(cornerRadius: 6))
                            }
                            Button { remove(b) } label: { Image(systemName: "minus.circle").foregroundStyle(Ink.bad) }
                                .buttonStyle(.plain).accessibilityLabel("Remove \(b.label)")
                        }
                        .contextMenu {
                            Button(b.afterPR ? "Show before a PR" : "Show only after a PR") { toggleAfterPR(b) }
                        }
                    }
                    .onMove { from, to in
                        list.move(fromOffsets: from, toOffset: to)
                        Task { await save() }
                    }
                } header: {
                    Text("Shown, in this order").font(Face.section)
                }
                Section {
                    ForEach(skills.filter { s in !list.contains { $0.skill == s.name } }, id: \.name) { s in
                        Button { add(s) } label: {
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(s.name).font(Face.mono(14, .medium)).foregroundStyle(Ink.ink)
                                    Text("\(s.description) · \(s.source)").font(Face.meta).foregroundStyle(Ink.mut).lineLimit(1)
                                }
                                Spacer()
                                Image(systemName: "plus.circle").foregroundStyle(Ink.ac)
                            }
                        }
                        .buttonStyle(.plain)
                    }
                    if skills.isEmpty {
                        Text("No running agent has reported its skills yet.").font(Face.meta).foregroundStyle(Ink.mut)
                    }
                } header: {
                    Text("Add from your agents' skills").font(Face.section)
                } footer: {
                    Text("Listed from what your agents report, so another owner sees their own skills here. Kept on the hub, so every paired phone shows the same buttons.")
                        .font(Face.meta)
                }
            }
            .environment(\.editMode, .constant(.active))
            .scrollContentBackground(.hidden)
        }
        .background(Ink.bg.ignoresSafeArea())
        .task { await model.loadShipButtons(); load() }
        .task { await loadSkills() }
        .onChange(of: scope) { _, _ in load() }
    }

    private var scopePicker: some View {
        HStack(spacing: 0) {
            segment("All projects", on: scope == "*") { scope = "*" }
            Menu {
                ForEach(projects, id: \.self) { p in Button(p) { scope = p } }
            } label: {
                Text(scope == "*" ? "One project" : "\(scope) only")
                    .font(Face.geist(15, .medium)).foregroundStyle(scope == "*" ? Ink.sub : Ink.ink)
                    .frame(maxWidth: .infinity, minHeight: 40)
                    .background(scope == "*" ? Color.clear : Ink.sf, in: RoundedRectangle(cornerRadius: 10))
            }
            .disabled(projects.isEmpty)
        }
        .padding(3)
        .background(Ink.fill, in: RoundedRectangle(cornerRadius: 12))
    }

    private func segment(_ title: String, on: Bool, _ tap: @escaping () -> Void) -> some View {
        Button(action: tap) {
            Text(title).font(Face.geist(15, .medium)).foregroundStyle(on ? Ink.ink : Ink.sub)
                .frame(maxWidth: .infinity, minHeight: 40)
                .background(on ? Ink.sf : Color.clear, in: RoundedRectangle(cornerRadius: 10))
        }
        .buttonStyle(.plain)
    }

    private func source(_ skill: String) -> String? { skills.first { $0.name == skill }?.source }

    private func load() {
        guard let b = model.shipButtons else { return }
        list = scope == "*" ? b.all.map(ShipButton.init) : (b.projects.first { $0.project == scope }?.buttons.map(ShipButton.init) ?? [])
    }

    private func add(_ s: Schemas.Skill) {
        list.append(ShipButton(skill: s.name, label: ShipButtons.label(for: s.name), afterPR: ShipButtons.afterPRByDefault(s.name)))
        Task { await save() }
    }

    private func remove(_ b: ShipButton) {
        list.removeAll { $0 == b }
        Task { await save() }
    }

    private func toggleAfterPR(_ b: ShipButton) {
        guard let i = list.firstIndex(of: b) else { return }
        list[i].afterPR.toggle()
        Task { await save() }
    }

    private func save() async {
        guard let hub = model.hub else { return }
        do {
            try await hub.setShipButtons(scope: scope, list)
            error = nil
            await model.loadShipButtons()
        } catch {
            self.error = error.localizedDescription
        }
    }

    /// The skills of up to three running agents, by name.
    private func loadSkills() async {
        guard let hub = model.hub else { return }
        var seen: [String: Schemas.Skill] = [:]
        let running = model.sessions.filter { $0.lifecycle == "running" }.prefix(3)
        for s in running {
            for k in (try? await hub.skills(s.id)) ?? [] where seen[k.name] == nil { seen[k.name] = k }
        }
        if seen.isEmpty {
            for a in model.agents.filter(\.isNamed).prefix(3) {
                for k in (try? await hub.agentSkills(machine: a.machineId, agent: a.name)) ?? [] where seen[k.name] == nil { seen[k.name] = k }
            }
        }
        skills = seen.values.sorted { $0.name < $1.name }
    }
}
