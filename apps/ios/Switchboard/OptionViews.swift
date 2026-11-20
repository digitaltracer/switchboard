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

/// Options as radio rows, shared by hook questions and dialogs on an agent's screen. One not on
/// offer is drawn dashed and can't be picked.
struct OptionList: View {
    let options: [OptionItem]
    var selected: Set<String> = []
    var multi = false
    var disabled = false
    let tap: (OptionItem) -> Void

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(options.enumerated()), id: \.element.id) { i, o in
                Button { tap(o) } label: {
                    RadioRow(title: o.label, on: selected.contains(o.id), offered: o.offered, first: i == 0) {
                        if let d = o.detail, !d.isEmpty, d != o.label { Note(text: d) }
                        if !o.offered { Note(text: "Answered at its terminal, not from the phone") }
                    }
                }
                .buttonStyle(.plain)
                .disabled(!o.offered || disabled)
                .accessibilityIdentifier("option-\(o.label)")
            }
        }
    }
}

/// A group of questions as chips: the open one filled, the answered ones ticked. They show
/// progress; they aren't buttons.
struct StepTabs: View {
    let steps: [Schemas.DialogStep]

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(Array(steps.enumerated()), id: \.offset) { i, s in
                    Chip(text: s.answered ? "✓ \(s.label)" : s.label, on: s.current)
                        .accessibilityIdentifier(s.current ? "step-current-\(s.label)" : "step-\(s.label)")
                }
            }
            .padding(.horizontal, 20)
        }
        .padding(.horizontal, -20)
    }
}

/// Where the decision page for an agent's dialog opens from.
struct DecisionRef: Hashable {
    let machine: String
    let agent: String
    let label: String
    /// `Claude Code · mac`.
    var meta: String?
}

/// The dialog on an agent's screen, followed while shown, as the Agent page's card links it.
@MainActor
@Observable
final class DialogWatch {
    let machine: String
    let agent: String
    private(set) var dialog: Schemas.Dialog?
    /// Waiting on something the phone can't show as options.
    private(set) var waitingElsewhere = false
    private(set) var loaded = false

    init(machine: String, agent: String) {
        self.machine = machine
        self.agent = agent
    }

    func follow(_ model: AppModel) async {
        guard model.followKey != nil else { return }
        while !Task.isCancelled {
            if let hub = model.hub, let d = try? await hub.dialog(machine: machine, agent: agent) {
                dialog = d.dialog
                waitingElsewhere = d.dialog == nil && d.waiting == true
                loaded = true
            }
            try? await Task.sleep(for: .seconds(dialog != nil || waitingElsewhere ? 3 : 5))
        }
    }
}

/// On the Agent page: the agent is waiting, with a way to answer.
struct WaitingCard: View {
    let watch: DialogWatch
    let ref: DecisionRef
    var screen: () -> Void

    var body: some View {
        if let d = watch.dialog {
            CardBox {
                HStack(spacing: 10) {
                    StatusGlyph(glyph: .need)
                    Text(d.steps?.isEmpty == false ? "It has \(d.steps?.filter { !$0.label.lowercased().hasPrefix("submit") }.count ?? 0) questions" : "It's waiting for you")
                        .font(Face.title).foregroundStyle(Ink.warn)
                }
                Text(d.prompt).font(Face.say).foregroundStyle(Ink.ink).fixedSize(horizontal: false, vertical: true)
                NavigationLink(value: ref) { Text(d.answerable ? "Answer" : "See the question") }
                    .buttonStyle(BigButtonStyle())
                    .accessibilityIdentifier("answerDialog")
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("dialog-waiting")
        } else if watch.waitingElsewhere {
            CardBox {
                HStack(spacing: 10) {
                    StatusGlyph(glyph: .need)
                    Text("It's waiting for you").font(Face.title).foregroundStyle(Ink.warn)
                }
                Note(text: "On something the phone can't show as options. Look at its screen, or answer it at the machine.", color: Ink.sub)
                Button("See its screen", action: screen).buttonStyle(BigButtonStyle(kind: .outline))
            }
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("dialog-elsewhere")
        }
    }
}

/// A decision as its own page: the question as the headline, its options as radio rows, one
/// button at the bottom. A tap picks an option and the button sends it; any option but the deny
/// asks for Face ID. In a group, each pick is sent with Next, and only the one that submits them
/// asks for Face ID. A screen it doesn't recognise is shown read-only.
struct DecisionPage: View {
    @Environment(AppModel.self) private var model
    let ref: DecisionRef
    @State private var dialog: Schemas.Dialog?
    @State private var waitingElsewhere = false
    @State private var loaded = false
    @State private var busy = false
    @State private var result: String?
    /// The option picked on the phone, sent only with the button.
    @State private var picked: String?
    /// Cancel on a group's last step was tapped once; a second tap declines every question.
    @State private var cancelArmed = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            PageTop(back: "Back", meta: ref.meta)
            if let dialog { page(dialog) } else { empty }
        }
        .inboxPage()
        .task(id: model.followKey) {
            guard model.followKey != nil else { return }
            while !Task.isCancelled {
                await load()
                try? await Task.sleep(for: .seconds(3))
            }
        }
    }

    @ViewBuilder
    private func page(_ dialog: Schemas.Dialog) -> some View {
        let last = Self.submitStep(dialog)
        let permission = dialog.steps?.isEmpty != false && dialog.options.contains(where: \.isDeny)
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 10) {
                    StatusGlyph(glyph: .need)
                    Text(permission ? "\(ref.label) asks permission" : "\(ref.label) is waiting")
                        .font(Face.geist(15, .medium))
                        .foregroundStyle(Ink.warn)
                        .lineLimit(2)
                }
                .padding(.top, 6)
                if let steps = dialog.steps, !steps.isEmpty { StepTabs(steps: steps).padding(.top, 16) }
                let (question, detail) = Self.split(dialog.prompt)
                Headline(last != nil ? "Ready to submit your answers?" : question)
                    .stateStyle(size: 30)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 22)
                if last != nil, dialog.prompt != "Ready to submit your answers?" {
                    Note(text: dialog.prompt, color: Ink.sub).padding(.top, 10)
                } else if let detail {
                    if permission { CodeBlock(text: detail, wraps: true).padding(.top, 18) } else { Note(text: detail, color: Ink.sub).padding(.top, 10) }
                }
                if last == nil {
                    OptionList(
                        options: dialog.options.map {
                            OptionItem(id: $0.id, label: $0.label, offered: $0.offered && dialog.answerable, isDeny: $0.isDeny)
                        },
                        selected: Set([picked].compactMap { $0 }),
                        disabled: busy
                    ) { o in picked = o.id }
                    .padding(.top, 22)
                    Hairline()
                } else if let steps = dialog.steps {
                    VStack(spacing: 0) {
                        ForEach(Array(steps.filter(\.answered).enumerated()), id: \.offset) { i, s in
                            Row(title: s.label, subtitle: "Answered", first: i == 0)
                        }
                    }
                    .padding(.top, 22)
                    Hairline()
                }
            }
            .padding(.horizontal, 20)
            .padding(.bottom, 24)
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("dialog-card")
        VStack(alignment: .leading, spacing: 10) {
            Note(text: dialog.answerable ? Self.hint(dialog) : Self.why(dialog.reason))
            if let result { Note(text: result, color: Ink.sub).accessibilityIdentifier("dialog-result") }
            if dialog.answerable { buttons(dialog, last: last) }
        }
        .padding(.horizontal, 20)
        .padding(.bottom, 12)
    }

    @ViewBuilder
    private func buttons(_ dialog: Schemas.Dialog, last: (submit: Schemas.DialogOption, cancel: Schemas.DialogOption?)?) -> some View {
        let chosen = dialog.options.first { $0.id == picked }
        // One question of a group submits nothing, so it goes without Face ID.
        let next = chosen?.provisional == true
        HStack(spacing: 10) {
            if dialog.canGoBack == true {
                Button { Task { await back() } } label: { Text("Back") }
                    .buttonStyle(BigButtonStyle(kind: .outline))
                    .frame(maxWidth: 140)
                    .disabled(busy)
                    .accessibilityIdentifier("dialog-back")
            }
            if let last {
                Button { Task { await answer(last.submit.id) } } label: { Label(last.submit.label, systemImage: "faceid") }
                    .buttonStyle(BigButtonStyle())
                    .disabled(busy)
                    .accessibilityIdentifier("dialog-confirm")
            } else {
                Button { if let picked { Task { await answer(picked) } } } label: {
                    if next {
                        Text("Next question")
                    } else if chosen?.isDeny == true {
                        Text("Confirm no")
                    } else {
                        Label("Confirm", systemImage: "faceid")
                    }
                }
                .buttonStyle(BigButtonStyle())
                .disabled(picked == nil || busy)
                .accessibilityIdentifier("dialog-confirm")
            }
        }
        if let cancel = last?.cancel {
            Button(cancelArmed ? "Tap again to decline every question" : "Cancel and decline every question") {
                if cancelArmed { Task { await answer(cancel.id) } } else { cancelArmed = true }
            }
            .buttonStyle(BigButtonStyle(kind: .plain))
            .foregroundStyle(cancelArmed ? Ink.bad : Ink.sub)
            .disabled(busy)
            .accessibilityIdentifier("dialog-cancel")
        }
    }

    private var empty: some View {
        VStack(alignment: .leading, spacing: 12) {
            if !loaded {
                StateRow(text: "Reading its screen…", busy: true)
            } else if waitingElsewhere {
                Headline("It's waiting on something else.").stateStyle(size: 30).fixedSize(horizontal: false, vertical: true)
                Note(text: "Switchboard can't show it as options here. Look at its screen, or answer it at the machine.", color: Ink.sub)
            } else {
                Headline("Nothing to answer now.").stateStyle(size: 30)
            }
            if let result { Note(text: result, color: Ink.sub).accessibilityIdentifier("dialog-result") }
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.top, 22)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier(waitingElsewhere ? "dialog-elsewhere" : "dialog-none")
    }

    /// The question, and what it's about: `Would you like to run the following command?` and
    /// `cargo test --workspace`.
    static func split(_ prompt: String) -> (String, String?) {
        let lines = prompt.split(separator: "\n", omittingEmptySubsequences: true).map { $0.trimmingCharacters(in: .whitespaces) }
        guard let first = lines.first, lines.count > 1 else { return (prompt, nil) }
        let rest = lines.dropFirst().map { $0.hasPrefix("$ ") ? String($0.dropFirst(2)) : $0 }.joined(separator: "\n")
        return (first, rest)
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
        if dialog.steps?.isEmpty == false { return "Pick an option for each question. Face ID is asked once, when you submit." }
        return dialog.options.contains(where: \.isDeny)
            ? "Saying no needs no Face ID. Any other answer asks for it."
            : "Pick an option, then confirm. Face ID is asked when you do."
    }

    static func why(_ reason: String?) -> String {
        switch reason {
        case "hooked": return "Answer it as the approval on its session."
        case "answered": return "Already answered from the phone."
        case "read_only": return "This kind of question can't be answered from the phone. Answer it at its terminal."
        case "hook_pending", "hook_failed": return "Its approval is being handled; answer it at its terminal if it stays."
        default: return "Answer it at its terminal, on its machine."
        }
    }

    private func load() async {
        guard let hub = model.hub, !busy else { return }
        // A failed read keeps what is shown.
        guard let d = try? await hub.dialog(machine: ref.machine, agent: ref.agent) else { return }
        if d.dialog?.occurrenceId != dialog?.occurrenceId {
            if dialog != nil, d.dialog != nil { result = nil }
            picked = nil
            cancelArmed = false
        }
        dialog = d.dialog
        waitingElsewhere = d.dialog == nil && d.waiting == true
        loaded = true
    }

    private func back() async {
        guard let hub = model.hub, let dialog else { return }
        busy = true
        var shown = false
        do {
            let previous = try await hub.back(machine: ref.machine, agent: ref.agent, dialog: dialog)
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
                machine: ref.machine, agent: ref.agent, dialog: dialog, option: option,
                reason: "Answer \(ref.label): \(option.label)", requestId: UUID().uuidString
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
