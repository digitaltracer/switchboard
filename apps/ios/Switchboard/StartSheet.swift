import SwiftUI
import SwitchboardKit

/// New session: the prompt, typed or dictated; whether it opens in the Herdr app or a Herdr
/// terminal; and the project, machine and agent. A lost answer becomes "outcome unknown" and is
/// reconciled by request id; the same start within a minute comes back as "already running".
struct StartSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(Voice.self) private var voice
    let done: (String?) -> Void
    /// A Herdr app space the session's tab opens in, instead of a new space.
    struct Into {
        let spaceId: String
        let title: String
        let machine: String
    }
    var into: Into?
    @State private var projects: [Schemas.Project] = []
    @State private var machines: [Schemas.Machine] = []
    @State private var project: String
    @State private var machine = ""
    @State private var agent = ""
    @State private var prompt = ""
    @State private var notify = true
    /// `hc` or `herdr`, once the machine has said what it can take.
    @State private var backend: String?
    @State private var options: Schemas.StartOptions?
    @State private var optionsError: String?
    /// The hub predates choosing where a session opens.
    @State private var hubTooOld = false
    @State private var state: StartState?
    @State private var loadError: String?
    @State private var sending = false
    @State private var startTask: Task<Void, Never>?
    @FocusState private var focused: Bool

    init(project: String? = nil, into: Into? = nil, done: @escaping (String?) -> Void) {
        self.done = done
        self.into = into
        _project = State(initialValue: project ?? "")
    }

    private var blank: Bool { prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    /// The machine's runner can be told where to open it.
    private var canChoose: Bool { options?._default != nil }

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                // A start still reconciling stops being watched; the hub still settles it.
                Button("Cancel") { startTask?.cancel(); voice.cancel(); done(nil) }
                    .font(Face.geist(16, .medium))
                    .foregroundStyle(Ink.ac)
                    .frame(minHeight: 44)
                Spacer()
                Text("New session").font(Face.meta).foregroundStyle(Ink.mut)
            }
            .padding(.horizontal, 20)
            .frame(height: 44)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    Headline("Start something new.").stateStyle().padding(.top, 8)
                    if let loadError { Note(text: loadError, color: Ink.bad).padding(.top, 10) }
                    Text("What should the agent do?").font(Face.meta).foregroundStyle(Ink.mut).padding(.top, 14).padding(.bottom, 8)
                    promptBox
                    dictationState
                    SectionHead("Open it in").padding(.top, -6)
                    if let into { fixedSpace(into) } else { backends }
                    SectionHead("What runs").padding(.top, -6)
                    fields
                    HStack {
                        Text("Notify me when it needs me").font(Face.geist(16, relativeTo: .body)).foregroundStyle(Ink.ink)
                        Spacer()
                        Toggle("", isOn: $notify).labelsHidden().tint(Ink.ink)
                    }
                    .frame(minHeight: 50)
                    .overlay(alignment: .top) { Hairline() }
                    Hairline()
                    if let state { StartStatus(state: state, open: done).padding(.top, 14) }
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 16)
            }
            .scrollDismissesKeyboard(.interactively)
            Button { startTask = Task { await start() } } label: {
                if sending { ProgressView().tint(Ink.inv) } else { Text("Start") }
            }
            .buttonStyle(BigButtonStyle())
            .disabled(project.isEmpty || blank || sending || voice.dictating || (into != nil && !appOpen))
            .padding(.horizontal, 20)
            .padding(.bottom, 8)
            .accessibilityIdentifier("submitStart")
        }
        .background(Ink.bg.ignoresSafeArea())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("newSession")
        .overlay { if voice.dictating { VoiceOverlay() } }
        .presentationDragIndicator(.visible)
        .interactiveDismissDisabled(voice.dictating)
        .task { await load() }
        .task(id: machine) { await loadOptions() }
        .onChange(of: project) { _, id in
            agent = projects.first { $0.id == id }?.defaultAgent ?? "claude"
            let available = projects.first { $0.id == id }?.machines ?? []
            if into == nil, !available.contains(machine) { machine = available.first ?? "" }
        }
        // Dictation ends in review; its words come here, where they stay editable.
        .onChange(of: voice.phase) { _, phase in
            if phase == .review, voice.dictating, !voice.retrying, !voice.canRetry { takeDictation() }
        }
        .onChange(of: voice.retrying) { _, retrying in
            if !retrying, voice.phase == .review, voice.dictating, !voice.canRetry { takeDictation() }
        }
    }

    private var promptBox: some View {
        ZStack(alignment: .bottomTrailing) {
            TextField("Fix the flaky retry test and tell me why it failed", text: $prompt, axis: .vertical)
                .font(Face.geist(16, relativeTo: .body))
                .lineSpacing(4)
                .lineLimit(4...8)
                .focused($focused)
                .padding(.leading, 16)
                .padding(.trailing, 60)
                .padding(.vertical, 14)
                .frame(minHeight: 96, alignment: .topLeading)
                .accessibilityIdentifier("promptField")
            Button { focused = false; voice.dictate(after: prompt) } label: { MicDisc() }
                .buttonStyle(.plain)
                .padding(10)
                .accessibilityLabel("Dictate the prompt")
                .accessibilityIdentifier("dictateButton")
        }
        .background(Ink.sf, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Ink.line))
    }

    /// A dictation that failed waits here for Retry; its words so far stay.
    @ViewBuilder
    private var dictationState: some View {
        if voice.dictating, voice.phase == .review {
            VStack(alignment: .leading, spacing: 8) {
                if let error = voice.error { Note(text: error, color: Ink.warn) }
                if voice.retrying {
                    HStack(spacing: 8) { ProgressView(); Note(text: "Transcribing with OpenAI…") }
                        .accessibilityIdentifier("voiceRetrying")
                } else {
                    HStack(spacing: 10) {
                        if voice.canRetry {
                            Button("Retry") { voice.retry() }
                                .buttonStyle(BigButtonStyle(kind: .outline))
                                .accessibilityIdentifier("voiceRetry")
                        }
                        Button("Keep what I have") { takeDictation() }
                            .buttonStyle(BigButtonStyle(kind: .plain))
                            .foregroundStyle(Ink.sub)
                    }
                }
            }
            .padding(.top, 10)
        }
    }

    @ViewBuilder
    private var backends: some View {
        let reasonless = options == nil && optionsError == nil && !hubTooOld
        VStack(spacing: 0) {
            ForEach(["hc", "herdr"], id: \.self) { b in
                let opt = options?.backends.first { $0.backend == b }
                let offered = canChoose && opt?.available == true
                Button { backend = b } label: {
                    RadioRow(title: b == "hc" ? "Herdr app" : "Herdr terminal", on: canChoose && backend == b, offered: offered || reasonless,
                             first: b == "hc") {
                        Note(text: describe(b, opt))
                    }
                }
                .buttonStyle(.plain)
                .disabled(!offered)
                .accessibilityIdentifier("backend-\(b)")
            }
            Hairline()
        }
    }

    /// The Herdr app answers on the machine, so a session can open in one of its spaces.
    private var appOpen: Bool { options?.backends.first { $0.backend == "hc" }?.available == true }

    /// The space a start from its page goes to; there is no choice to make.
    @ViewBuilder
    private func fixedSpace(_ into: Into) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            FieldRow("Space", value: into.title, first: true)
            if hubTooOld || optionsError != nil || (options != nil && !appOpen) {
                Note(text: describe("hc", options?.backends.first { $0.backend == "hc" }), color: Ink.warn)
                    .padding(.bottom, 11)
            } else {
                Note(text: "A new tab in this space in the Herdr window on \(into.machine), on its own branch.")
                    .padding(.bottom, 11)
            }
            Hairline()
        }
        .accessibilityIdentifier("startInto")
    }

    private func describe(_ b: String, _ o: Schemas.BackendOption?) -> String {
        let m = machine.isEmpty ? "the machine" : machine
        if hubTooOld {
            return b == "hc" ? "Your hub is too old to choose. The session opens where \(m) is set to." : "Update the hub to choose."
        }
        if let optionsError { return "Couldn't ask \(m): \(optionsError)" }
        guard let options, let o else { return "Checking \(m)…" }
        if options._default == nil {
            return b == "hc" ? "\(m)’s runner is too old to choose. The session opens where it’s set to." : "Update the runner on \(m) to choose."
        }
        if o.available {
            return b == "hc"
                ? "A new space in the Herdr window on \(m), on its own branch. The app is open there now."
                : "Its own Herdr session. Attach to it from any terminal on \(m)."
        }
        switch o.reason {
        case "not_answering": return "The Herdr app isn’t open on \(m). Open it there to start here."
        case "not_followed": return "\(m)’s runner doesn’t use the Herdr app."
        default: return "\(m) can’t open it here now."
        }
    }

    @ViewBuilder
    private var fields: some View {
        VStack(spacing: 0) {
            Menu {
                Picker("Project", selection: $project) {
                    ForEach(projects, id: \.id) { Text($0.id).tag($0.id) }
                }
            } label: { field("Project") { Text(project.isEmpty ? "Choose" : project) } }
            .accessibilityIdentifier("projectPicker")
            Menu {
                Picker("Machine", selection: $machine) {
                    ForEach(eligible, id: \.id) { m in Text("\(m.id) · \(m.status)").tag(m.id) }
                }
            } label: {
                field("Machine") {
                    let status = machines.first { $0.id == machine }?.status
                    HStack(spacing: 8) {
                        Circle().fill(status == "ready" ? Ink.ok : Ink.warn).frame(width: 8, height: 8)
                        Text(machine.isEmpty ? "Choose" : "\(machine) · \(status ?? "unknown")")
                    }
                }
            }
            .accessibilityIdentifier("machinePicker")
            Menu {
                Picker("Agent", selection: $agent) {
                    Text("Claude Code").tag("claude")
                    Text("Codex").tag("codex")
                }
            } label: { field("Agent") { Text(agent == "codex" ? "Codex" : "Claude Code") } }
            .accessibilityIdentifier("agentPicker")
        }
    }

    private var eligible: [Schemas.Machine] {
        machines.filter { m in
            // A space is on one machine; a start into it runs there or not at all.
            if let into { return m.id == into.machine }
            return projects.first { $0.id == project }?.machines.contains(m.id) ?? true
        }
    }

    private func field<V: View>(_ label: String, @ViewBuilder value: () -> V) -> some View {
        HStack(spacing: 12) {
            Text(label).font(Face.geist(16, relativeTo: .body)).foregroundStyle(Ink.ink)
            Spacer(minLength: 8)
            HStack(spacing: 8) {
                value()
                Chevron(down: true)
            }
            .font(Face.geist(16, relativeTo: .body))
            .foregroundStyle(Ink.sub)
            .lineLimit(1)
        }
        .frame(minHeight: 50)
        .overlay(alignment: .top) { Hairline() }
        .contentShape(Rectangle())
    }

    private func takeDictation() {
        let words = voice.text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !words.isEmpty { prompt = words }
        voice.finish()
    }

    private func load() async {
        guard let hub = model.hub else { return }
        do {
            projects = try await hub.projects()
            machines = try await hub.machines()
            // Only projects with a repo on the space's machine can start there.
            if let into { projects = projects.filter { $0.machines.contains(into.machine) } }
            if project.isEmpty || !projects.contains(where: { $0.id == project }), let first = projects.first { project = first.id }
            agent = projects.first { $0.id == project }?.defaultAgent ?? "claude"
            let available = projects.first { $0.id == project }?.machines ?? []
            let preferred = machines.first(where: \._default)?.id ?? machines.first?.id ?? ""
            machine = into?.machine ?? (available.contains(preferred) ? preferred : available.first ?? preferred)
        } catch {
            loadError = error.localizedDescription
        }
    }

    /// Asks the chosen machine where a session would open; picks that, or the other if it can't.
    private func loadOptions() async {
        options = nil
        optionsError = nil
        hubTooOld = false
        backend = nil
        guard let hub = model.hub, !machine.isEmpty else { return }
        do {
            let o = try await hub.startOptions(machine: machine)
            guard !Task.isCancelled else { return }
            options = o
            let available = o.backends.filter(\.available).map(\.backend)
            backend = into != nil ? "hc" : o._default.flatMap { available.contains($0) ? $0 : nil } ?? available.first
        } catch let e as HubError where e.code == "http_404" {
            hubTooOld = true
        } catch {
            if !Task.isCancelled { optionsError = error.localizedDescription }
        }
    }

    private func start() async {
        guard let hub = model.hub else { return }
        sending = true
        defer { sending = false }
        let requestId = UUID().uuidString
        state = await StartFlow.run(hub: hub, project: project, prompt: prompt, machine: machine.isEmpty ? nil : machine,
                                    agent: agent, notify: notify, requestId: requestId,
                                    backend: into != nil ? "hc" : canChoose ? backend : nil, spaceId: into?.spaceId)
        // Reconcile an unknown outcome by asking the hub about the request, for up to five minutes.
        var tries = 0
        while case .unknown = state, tries < 150, !Task.isCancelled {
            try? await Task.sleep(for: .seconds(2))
            state = await StartFlow.reconcile(hub: hub, requestId: requestId, attempt: tries)
            tries += 1
        }
        if !Task.isCancelled, case .started(let id, false) = state { done(id) }
    }
}

struct StartStatus: View {
    let state: StartState
    let open: (String?) -> Void

    var body: some View {
        switch state {
        case .started(let id, true):
            VStack(alignment: .leading, spacing: 10) {
                Note(text: "Already running: the same start was sent in the last minute.", color: Ink.sub)
                Button("Open it") { open(id) }
                    .buttonStyle(BigButtonStyle(kind: .outline))
                    .accessibilityIdentifier("openDuplicate")
            }
        case .started:
            Note(text: "Started.", color: Ink.ok)
        case .unknown:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Note(text: "Outcome unknown; checking with the hub…", color: Ink.sub)
            }
            .accessibilityIdentifier("reconciling")
        case .failed(let message):
            Note(text: message, color: Ink.bad).accessibilityIdentifier("startFailed")
        }
    }
}
