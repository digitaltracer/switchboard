import SwiftUI
import SwitchboardKit

/// One text field for a new name. Save stays off until the name fits; a failure stays in the
/// sheet, in words, with what was typed.
struct RenameSheet: View {
    let title: String
    let current: String
    /// An agent in a Herdr terminal, whose names Herdr restricts.
    var herdrAgent = false
    /// Sends the trimmed name; throws to keep the sheet open.
    let save: (String) async throws -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var name: String
    @State private var error: String?
    @State private var saving = false
    @FocusState private var focused: Bool

    init(title: String, current: String, herdrAgent: Bool = false, save: @escaping (String) async throws -> Void) {
        self.title = title
        self.current = current
        self.herdrAgent = herdrAgent
        self.save = save
        _name = State(initialValue: current)
    }

    private var trimmed: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var problem: String? { Names.problem(name, herdrAgent: herdrAgent) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title)
                .font(Face.h1)
                .tracking(-0.84)
                .foregroundStyle(Ink.ink)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            TextField("Name", text: $name)
                .font(Face.geist(16, relativeTo: .body))
                .textInputAutocapitalization(herdrAgent ? .never : .sentences)
                .autocorrectionDisabled()
                .submitLabel(.done)
                .focused($focused)
                .onSubmit { Task { await send() } }
                .padding(.horizontal, 16)
                .frame(minHeight: 52)
                .background(Ink.bg, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Ink.line))
                .padding(.top, 16)
                .accessibilityIdentifier("renameField")
            if let error {
                Note(text: error, color: Ink.bad).padding(.top, 10).accessibilityIdentifier("renameError")
            } else if herdrAgent {
                Note(text: Names.herdrAgentRule).padding(.top, 10)
            } else if !trimmed.isEmpty, let problem {
                Note(text: problem, color: Ink.warn).padding(.top, 10)
            }
            VStack(spacing: 10) {
                Button { Task { await send() } } label: {
                    if saving { ProgressView().tint(Ink.inv) } else { Text("Save") }
                }
                .buttonStyle(BigButtonStyle())
                .disabled(problem != nil || trimmed == current || saving)
                .accessibilityIdentifier("renameSave")
                Button("Cancel") { dismiss() }
                    .buttonStyle(BigButtonStyle(kind: .plain))
                    .foregroundStyle(Ink.sub)
            }
            .padding(.top, 16)
        }
        .padding(.horizontal, 20)
        .padding(.top, 14)
        .padding(.bottom, 8)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Ink.sf)
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationCornerRadius(28)
        .presentationBackground(Ink.sf)
        .onAppear { focused = true }
    }

    private func send() async {
        guard problem == nil, trimmed != current, !saving else { return }
        saving = true
        defer { saving = false }
        do {
            try await save(trimmed)
            dismiss()
        } catch {
            self.error = Names.failure(error)
        }
    }
}

/// A "Name" row under "At your desk": the name with Rename beside it, or why it can't be
/// renamed from the phone. Nothing when there is nothing to offer or say.
struct NameRow: View {
    let name: String
    let offer: Offer
    let rename: () -> Void

    var body: some View {
        switch offer {
        case .yes:
            FieldRow(label: "Name") {
                HStack(spacing: 12) {
                    Text(name)
                    Button("Rename", action: rename)
                        .font(Face.geist(15, .medium))
                        .foregroundStyle(Ink.ac)
                        .frame(minHeight: 44)
                        .accessibilityIdentifier("rename")
                }
            }
        case .no(let why):
            FieldRow("Name", value: name)
            Note(text: why, color: Ink.sub).padding(.bottom, 11).accessibilityIdentifier("renameBlocked")
        case .hidden:
            EmptyView()
        }
    }
}

/// Long-press on an agent's or a space's row: Rename, or why it can't be renamed from the phone.
struct RenameMenu: ViewModifier {
    @Environment(AppModel.self) private var model
    let name: String
    let offer: Offer
    var herdrAgent = false
    /// Sends the new name with its request id; throws to keep the sheet open.
    let save: (AppModel, String, String) async throws -> Void
    @State private var renaming = false
    /// One request id per name tried, so a retry after a lost answer is the same request.
    @State private var ids: [String: String] = [:]

    @ViewBuilder
    func body(content: Content) -> some View {
        switch offer {
        case .hidden:
            content
        case .yes, .no:
            // A row has no fill of its own; give the lifted copy one, a little wider than the text.
            content
                .padding(.horizontal, 12)
                .background(Ink.bg)
                .contentShape(.contextMenuPreview, RoundedRectangle(cornerRadius: 16, style: .continuous))
                .padding(.horizontal, -12)
                .contextMenu {
                    if case .no(let why) = offer {
                        Button {} label: { Text("Rename"); Text(why) }.disabled(true)
                    } else {
                        Button { renaming = true } label: { Label("Rename", systemImage: "pencil") }
                            .accessibilityIdentifier("renameMenu")
                    }
                }
                .sheet(isPresented: $renaming) {
                    RenameSheet(title: "Rename \(name)", current: name, herdrAgent: herdrAgent) { new in
                        let id = ids[new] ?? UUID().uuidString
                        ids[new] = id
                        try await save(model, new, id)
                    }
                }
        }
    }
}

extension View {
    func renameMenu(agent: HerdrSession.Agent) -> some View {
        let name = if case .task(let s) = agent { s.label } else { agent.nameInSpace }
        return modifier(RenameMenu(name: name, offer: agent.rename, herdrAgent: agent.isHerdrAgent) { model, new, id in
            try await model.rename(agent, to: new, requestId: id)
        })
    }

    func renameMenu(space: AgentSpace) -> some View {
        modifier(RenameMenu(name: space.key.space ?? space.title, offer: space.rename) { model, new, id in
            try await model.rename(space, to: new, requestId: id)
        })
    }
}
