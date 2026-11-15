import SwiftUI
import SwitchboardKit

/// One choice an agent waits on, as a row: a hook question's option or a dialog's.
struct OptionItem: Identifiable, Hashable {
    let id: String
    let label: String
    var detail: String?
    /// Shown for context only when false: free text or a chat.
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

/// A group of questions as tabs: the open one filled, the answered ones ticked.
struct StepTabs: View {
    let steps: [Schemas.DialogStep]

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(Array(steps.enumerated()), id: \.offset) { _, s in
                    HStack(spacing: 4) {
                        if s.answered { Image(systemName: "checkmark").font(.caption2.weight(.bold)) }
                        Text(s.label).font(.footnote.weight(s.current ? .semibold : .regular))
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(s.current ? Color.accentColor : Color(.tertiarySystemFill), in: Capsule())
                    .foregroundStyle(s.current ? Color.white : s.answered ? Color.primary : Color.secondary)
                    .accessibilityIdentifier(s.current ? "step-current-\(s.label)" : "step-\(s.label)")
                }
            }
        }
    }
}

/// The dialog on an agent's screen, answered from the phone when Switchboard recognises it. A tap
/// picks an option and Confirm sends it; any option but the deny asks for Face ID. In a group of
/// questions each pick is sent with Next, and only the one that submits them asks for Face ID.
/// It reads the screen itself while shown, so a dialog that
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
    /// The option picked on the phone, sent only on Confirm.
    @State private var picked: String?
    /// Cancel on a group's last step was tapped once; a second tap declines every question.
    @State private var cancelArmed = false

    var body: some View {
        VStack(spacing: 0) {
            // Always here, so the read below runs before there is a card to show.
            Color.clear.frame(height: 0)
            if let dialog {
                VStack(alignment: .leading, spacing: 12) {
                    Label("\(label) is waiting", systemImage: "questionmark.bubble")
                        .font(.headline)
                        .foregroundStyle(Color.needs)
                    if let steps = dialog.steps, !steps.isEmpty { StepTabs(steps: steps) }
                    if let last = Self.submitStep(dialog) {
                        Text(dialog.prompt).font(.body)
                        if let result { Text(result).font(.footnote).foregroundStyle(.secondary) }
                        actions(dialog) {
                        Button { Task { await answer(last.submit.id) } } label: {
                            Label(last.submit.label, systemImage: "faceid")
                                .frame(maxWidth: .infinity, minHeight: 50)
                                .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                                .foregroundStyle(Color.white)
                        }
                        .font(.headline)
                        .buttonStyle(.plain)
                        .disabled(busy)
                        .accessibilityIdentifier("dialog-confirm")
                        }
                        if let cancel = last.cancel {
                            Button(cancelArmed ? "Tap again to decline every question" : cancel.label) {
                                if cancelArmed { Task { await answer(cancel.id) } } else { cancelArmed = true }
                            }
                            .font(.subheadline)
                            .foregroundStyle(cancelArmed ? Color.red : Color.secondary)
                            .frame(maxWidth: .infinity)
                            .disabled(busy)
                            .accessibilityIdentifier("dialog-cancel")
                        }
                    } else {
                    OptionList(
                        prompt: dialog.prompt,
                        options: dialog.options.map {
                            OptionItem(id: $0.id, label: $0.label, offered: $0.offered && dialog.answerable, isDeny: $0.isDeny)
                        },
                        selected: Set([picked].compactMap { $0 }),
                        disabled: busy
                    ) { o in picked = o.id }
                    Text(dialog.answerable ? Self.hint(dialog) : Self.why(dialog.reason))
                        .font(.footnote).foregroundStyle(.secondary)
                    if let result { Text(result).font(.footnote).foregroundStyle(.secondary) }
                    if dialog.answerable {
                        let chosen = dialog.options.first { $0.id == picked }
                        // One question of a group submits nothing, so it goes without Face ID.
                        let next = chosen?.provisional == true
                        actions(dialog) {
                        Button { if let picked { Task { await answer(picked) } } } label: {
                            Label(next ? "Next" : "Confirm",
                                  systemImage: next ? "arrow.right" : chosen?.isDeny == true ? "checkmark" : "faceid")
                                .frame(maxWidth: .infinity, minHeight: 50)
                                .background(picked == nil ? Color(.tertiarySystemFill) : Color.accentColor,
                                            in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                                .foregroundStyle(picked == nil ? Color.secondary : Color.white)
                        }
                        .font(.headline)
                        .buttonStyle(.plain)
                        .disabled(picked == nil || busy)
                        .accessibilityIdentifier("dialog-confirm")
                        }
                    }
                    }
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

    /// A step's main button, with Back beside it when the group has an earlier question.
    @ViewBuilder
    private func actions<Main: View>(_ dialog: Schemas.Dialog, @ViewBuilder main: () -> Main) -> some View {
        HStack(spacing: 10) {
            if dialog.canGoBack == true {
                Button { Task { await back() } } label: {
                    Label("Back", systemImage: "arrow.left")
                        .frame(minHeight: 50)
                        .padding(.horizontal, 16)
                        .background(Color(.tertiarySystemFill), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                }
                .font(.headline)
                .buttonStyle(.plain)
                .disabled(busy)
                .accessibilityIdentifier("dialog-back")
            }
            main()
        }
    }

    /// A group's last step when all it offers is to submit or cancel: drawn as buttons, not a list.
    static func submitStep(_ d: Schemas.Dialog) -> (submit: Schemas.DialogOption, cancel: Schemas.DialogOption?)? {
        guard d.answerable, d.steps?.isEmpty == false else { return nil }
        let offered = d.options.filter(\.offered)
        let submits = offered.filter { !$0.isDeny }
        guard submits.count == 1, submits[0].provisional != true, offered.count <= 2 else { return nil }
        return (submits[0], offered.first(where: \.isDeny))
    }

    static func hint(_ dialog: Schemas.Dialog) -> String {
        dialog.steps?.isEmpty == false
            ? "Pick an option for each question. Face ID is asked once, when you submit."
            : "Pick an option, then confirm."
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
        if d.dialog?.occurrenceId != dialog?.occurrenceId {
            if dialog != nil, d.dialog != nil { result = nil }
            picked = nil
            cancelArmed = false
        }
        dialog = d.dialog
        waitingElsewhere = d.dialog == nil && d.waiting == true
    }

    private func back() async {
        guard let hub = model.hub, let dialog else { return }
        busy = true
        var shown = false
        do {
            let previous = try await hub.back(machine: machine, agent: agent, dialog: dialog)
            result = nil
            if let previous { show(previous); shown = true }
        } catch let e as HubError where e.code == "dialog_changed" {
            result = "The question changed; here it is again."
        } catch {
            result = error.localizedDescription
        }
        busy = false
        if !shown { await load() }
    }

    /// Puts up a dialog the hub sent back with an answer or a step back, with no second read.
    private func show(_ next: Schemas.Dialog) {
        dialog = next
        waitingElsewhere = false
        picked = nil
        cancelArmed = false
    }

    private func answer(_ id: String) async {
        guard let hub = model.hub, let dialog, let option = dialog.options.first(where: { $0.id == id }) else { return }
        busy = true
        var shown = false
        do {
            let answered = try await hub.answer(
                machine: machine, agent: agent, dialog: dialog, option: option,
                reason: "Answer \(label): \(option.label)", requestId: UUID().uuidString
            )
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            if let next = answered.next {
                // A group's next question comes back with the answer.
                result = nil
                show(next)
                shown = true
            } else {
                result = "Answered: \(option.label)."
                self.dialog = nil
                picked = nil
            }
        } catch let e as HubError where e.code == "dialog_changed" {
            result = "The question changed; here it is again."
        } catch let e as HubError where e.outcomeUnknown {
            result = "The hub lost the connection; it may have been answered. Check the agent."
        } catch {
            result = error.localizedDescription
        }
        busy = false
        if !shown { await load() }
    }
}
