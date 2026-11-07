import SwiftUI
import SwitchboardKit

/// One choice an agent waits on, as a row: a hook question's option or a dialog's.
struct OptionItem: Identifiable, Hashable {
    let id: String
    let label: String
    var detail: String?
    /// Shown for context only when false: free text, a chat, or a standing permission.
    var offered = true
    var isDeny = false
}

/// The option card's list, shared by hook questions and dialogs on an agent's screen.
struct OptionList: View {
    let prompt: String
    let options: [OptionItem]
    var selected: Set<String> = []
    var disabled = false
    let tap: (OptionItem) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(prompt).font(.body)
            ForEach(options) { o in
                Button { tap(o) } label: {
                    HStack(alignment: .firstTextBaseline, spacing: 10) {
                        Image(systemName: selected.contains(o.id) ? "checkmark.circle.fill" : "circle")
                            .foregroundStyle(o.offered ? Color.accentColor : .secondary)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(o.label).font(.callout.weight(.medium)).multilineTextAlignment(.leading)
                            if let d = o.detail, !d.isEmpty, d != o.label {
                                Text(d).font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.leading)
                            }
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 10)
                    .background(Color(.tertiarySystemFill), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .opacity(o.offered ? 1 : 0.45)
                }
                .buttonStyle(.plain)
                .disabled(!o.offered || disabled)
                .accessibilityIdentifier("option-\(o.label)")
            }
        }
    }
}

/// The dialog on an agent's screen, answered from the phone when Switchboard recognises it. Any
/// option but the deny asks for Face ID. It reads the screen itself while shown, so a dialog that
/// appears after the page opened still shows, and says so when the agent waits on something else.
struct DialogCard: View {
    @Environment(AppModel.self) private var model
    let machine: String
    let agent: String
    let label: String
    @State private var dialog: Schemas.Dialog?
    /// Waiting on something the phone can't show as options.
    @State private var waitingElsewhere = false
    @State private var busy = false
    @State private var result: String?

    var body: some View {
        VStack(spacing: 0) {
            // Always here, so the read below runs before there is a card to show.
            Color.clear.frame(height: 0)
            if let dialog {
                VStack(alignment: .leading, spacing: 12) {
                    Label("\(label) is waiting", systemImage: "questionmark.bubble")
                        .font(.headline)
                        .foregroundStyle(Color.needs)
                    OptionList(
                        prompt: dialog.prompt,
                        options: dialog.options.map {
                            OptionItem(id: $0.id, label: $0.label, offered: $0.offered && dialog.answerable, isDeny: $0.isDeny)
                        },
                        selected: Set([dialog.selected].compactMap { $0 }),
                        disabled: busy
                    ) { o in Task { await answer(o.id) } }
                    Text(dialog.answerable ? "Tap an option to answer. The tick is what its screen has selected now." : Self.why(dialog.reason))
                        .font(.footnote).foregroundStyle(.secondary)
                    if let result { Text(result).font(.footnote).foregroundStyle(.secondary) }
                }
                .padding(16)
                .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 20, style: .continuous))
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("dialog-card")
            } else if waitingElsewhere || result != nil {
                VStack(alignment: .leading, spacing: 6) {
                    if waitingElsewhere {
                        Label("\(label) is waiting", systemImage: "questionmark.bubble")
                            .font(.headline)
                            .foregroundStyle(Color.needs)
                        Text("On something Switchboard can't show as options here. Check its screen, or answer it in Herdr.")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                    if let result { Text(result).font(.footnote).foregroundStyle(.secondary) }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(16)
                .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 20, style: .continuous))
                .accessibilityIdentifier("dialog-elsewhere")
            }
        }
        .task(id: model.followKey) {
            guard model.followKey != nil else { return }
            while !Task.isCancelled {
                await load()
                try? await Task.sleep(for: .seconds(dialog != nil || waitingElsewhere ? 3 : 5))
            }
        }
    }

    static func why(_ reason: String?) -> String {
        switch reason {
        case "hooked": return "Answer it as the approval above."
        case "answered": return "Already answered from the phone."
        case "read_only": return "This kind of question can't be answered from the phone. Answer it in Herdr."
        case "hook_pending", "hook_failed": return "Its approval is being handled; answer it in Herdr if it stays."
        default: return "Answer it in Herdr, on its machine."
        }
    }

    private func load() async {
        guard let hub = model.hub, !busy else { return }
        // A failed read keeps what is shown.
        guard let d = try? await hub.dialog(machine: machine, agent: agent) else { return }
        if d.dialog?.occurrenceId != dialog?.occurrenceId, dialog != nil, d.dialog != nil { result = nil }
        dialog = d.dialog
        waitingElsewhere = d.dialog == nil && d.waiting == true
    }

    private func answer(_ id: String) async {
        guard let hub = model.hub, let dialog, let option = dialog.options.first(where: { $0.id == id }) else { return }
        busy = true
        do {
            _ = try await hub.answer(
                machine: machine, agent: agent, dialog: dialog, option: option,
                reason: "Answer \(label): \(option.label)", requestId: UUID().uuidString
            )
            result = "Answered: \(option.label)."
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            self.dialog = nil
        } catch let e as HubError where e.code == "dialog_changed" {
            result = "The question changed; here it is again."
        } catch let e as HubError where e.outcomeUnknown {
            result = "The hub lost the connection; it may have been answered. Check the agent."
        } catch {
            result = error.localizedDescription
        }
        busy = false
        await load()
    }
}
