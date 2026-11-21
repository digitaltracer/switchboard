import SwiftUI
import SwitchboardKit

/// Review before sending: the words, heard or typed, and where they'll go. The hub's preview
/// picks the destination; the user can pick another.
struct SendSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(Voice.self) private var voice
    @FocusState private var focused: Bool
    @State private var preview: Schemas.MessagePreview?
    @State private var routes = RouteOptions.Result(options: [], suggested: nil)
    @State private var picked: String?
    /// A project picked under "New task in…", offered alongside the hub's options.
    @State private var newProject: String?
    @State private var note: String?
    @State private var sending = false
    /// One per draft and destination, so a retried send is never typed twice.
    @State private var requestId = UUID().uuidString
    /// The skills each agent can run, by target, fetched when `/` is first typed for it.
    @State private var skills: [String: [Schemas.Skill]] = [:]
    @State private var skillsError: String?

    private var current: Schemas.TargetView? { voice.context ?? model.current }
    private var shown: RouteOptions.Result { newProject.map { routes.withNewTask(in: $0) } ?? routes }
    private var chosen: RouteOption? { shown.options.first { $0.id == (picked ?? shown.suggested) } }
    private var unoffered: [String] { shown.projectsWithoutNewTask(model.projects.map(\.id)) }
    private var empty: Bool { voice.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var body: some View {
        @Bindable var voice = voice
        VStack(spacing: 0) {
            HStack {
                Button("Cancel") { voice.finish() }
                    .font(Face.geist(16, .medium))
                    .foregroundStyle(Ink.ac)
                    .frame(minHeight: 44)
                Spacer()
                Text(current.map { "To \($0.label)" } ?? "Review before sending")
                    .font(Face.meta).foregroundStyle(Ink.mut).lineLimit(1)
            }
            .padding(.horizontal, 20)
            .frame(height: 44)
            .padding(.top, 8)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    if let error = voice.error { Banner(text: error).padding(.bottom, 12) }
                    if let notice = voice.notice { Note(text: notice).padding(.bottom, 12) }
                    if voice.retrying {
                        HStack(spacing: 8) {
                            ProgressView()
                            Note(text: "Transcribing with OpenAI…")
                        }
                        .padding(.bottom, 12)
                        .accessibilityIdentifier("voiceRetrying")
                    } else if voice.canRetry {
                        Button("Retry") { voice.retry() }
                            .buttonStyle(BigButtonStyle(kind: .outline))
                            .padding(.bottom, 12)
                            .accessibilityIdentifier("voiceRetry")
                    }
                    TextField(current.map { "Message \($0.label)" } ?? "Say or type what you want done", text: $voice.text, axis: .vertical)
                        .font(SkillSearch.isTyping(voice.text) ? Face.mono(22, .medium) : Face.draft)
                        .tracking(SkillSearch.isTyping(voice.text) ? 0 : -0.44)
                        .foregroundStyle(Ink.ink)
                        .tint(Ink.ac)
                        .lineLimit(1...8)
                        .focused($focused)
                        .accessibilityIdentifier("composeField")
                    if !voice.text.isEmpty && !focused {
                        Note(text: "Tap the words to correct them.").padding(.top, 10)
                    }
                    if SkillSearch.isTyping(voice.text), let current { skillList(for: current) }
                    if !shown.options.isEmpty {
                        SectionHead(title: shown.options.count == 1 ? "Goes to" : "Where should this go?") { EmptyView() }
                            .padding(.top, 8)
                        if let message = preview?.message {
                            Note(text: message).padding(.bottom, 8).accessibilityIdentifier("previewMessage")
                        }
                        VStack(spacing: 0) {
                            ForEach(Array(shown.options.enumerated()), id: \.element.id) { i, o in
                                RouteRow(option: o, selected: o.id == chosen?.id, first: i == 0) {
                                    picked = o.id
                                    requestId = UUID().uuidString
                                }
                            }
                            if !unoffered.isEmpty {
                                NewTaskRow(projects: unoffered) { p in
                                    newProject = p
                                    picked = RouteOptions.newTask(p).id
                                    requestId = UUID().uuidString
                                }
                            }
                            Hairline()
                        }
                    } else if let preview, preview.outcome == "refused" {
                        Note(text: preview.message).padding(.top, 14)
                    }
                    if let note {
                        Note(text: note, color: Ink.sub).padding(.top, 12).accessibilityIdentifier("composeNote")
                    }
                }
                .padding(.horizontal, 20)
                .padding(.top, 8)
            }
            .scrollDismissesKeyboard(.interactively)
            HStack(spacing: 12) {
                Button { voice.listen(to: voice.context, keepText: true) } label: { RoundIcon(symbol: "mic") }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Keep talking")
                Button { Task { await send() } } label: {
                    if sending {
                        ProgressView().tint(Ink.inv)
                    } else {
                        Label(chosen?.verb ?? (empty ? "Say or type something" : "Pick where this goes"), systemImage: "arrow.up")
                    }
                }
                .buttonStyle(BigButtonStyle())
                .clipShape(Capsule())
                .disabled(chosen == nil || empty || sending || voice.retrying)
                .accessibilityIdentifier("composeSend")
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 8)
        }
        .background(Ink.sf.ignoresSafeArea())
        .presentationDetents([.large])
        .presentationCornerRadius(28)
        .presentationBackground(Ink.sf)
        .onAppear { if voice.text.isEmpty { focused = true } }
        .task(id: voice.text) { await loadPreview() }
    }

    private static func skillKey(_ t: Schemas.TargetView) -> String { AppModel.saidKey(for: t) }

    /// The agent's skills matching what's typed after `/`; picking one puts it in the message.
    @ViewBuilder
    private func skillList(for target: Schemas.TargetView) -> some View {
        let all = skills[Self.skillKey(target)]
        VStack(alignment: .leading, spacing: 0) {
            if let all {
                let shown = Array(SkillSearch.filter(all, typed: voice.text).prefix(30))
                SectionHead(title: "Skills matching \(voice.text.trimmingCharacters(in: .whitespaces))") { Text("\(shown.count)") }
                    .padding(.top, 8)
                if shown.isEmpty {
                    StateRow(text: "No skill matches \(voice.text).")
                }
                ForEach(Array(shown.enumerated()), id: \.element.name) { i, skill in
                    Button {
                        voice.text = skill.name + " "
                    } label: {
                        HStack(spacing: 12) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(skill.name).font(Face.mono(15, .medium)).foregroundStyle(Ink.ink)
                                if !skill.description.isEmpty {
                                    Text(skill.description).font(Face.sub).foregroundStyle(Ink.sub).lineLimit(2)
                                }
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            Text(Self.sourceLabel(skill.source)).font(Face.meta).foregroundStyle(Ink.mut)
                        }
                        .padding(.vertical, 12)
                        .frame(minHeight: 60)
                        .overlay(alignment: .top) { if i > 0 { Hairline() } }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("skill-\(skill.name)")
                }
                Hairline()
                Note(text: "Tapping a skill puts its name in the message. Nothing is sent until you tap Send.").padding(.top, 12)
            } else if let skillsError {
                StateRow(text: "Couldn't list its skills: \(skillsError)", error: true)
            } else {
                StateRow(text: "Listing its skills…", busy: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .task(id: Self.skillKey(target)) { await loadSkills(for: target) }
    }

    private static func sourceLabel(_ source: String) -> String {
        switch source {
        case "repo": "this repo"
        case "plugin": "plugin"
        case "builtin": "built in"
        case "user": "yours"
        default: source
        }
    }

    private func loadSkills(for t: Schemas.TargetView) async {
        let key = Self.skillKey(t)
        guard skills[key] == nil, let hub = model.hub else { return }
        do {
            let list = if let id = t.sessionId {
                try await hub.skills(id)
            } else if let agent = t.agent {
                try await hub.agentSkills(machine: t.machineId, agent: agent)
            } else {
                [Schemas.Skill]()
            }
            skills[key] = list
            skillsError = nil
        } catch {
            if !Task.isCancelled { skillsError = error.localizedDescription }
        }
    }

    /// Asks the hub where the text would go, after a short pause in typing.
    private func loadPreview() async {
        requestId = UUID().uuidString
        let text = voice.text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let hub = model.hub else {
            preview = nil
            routes = .init(options: [], suggested: nil)
            return
        }
        try? await Task.sleep(for: .milliseconds(300))
        guard !Task.isCancelled else { return }
        do {
            let p = try await hub.preview(text, current: current)
            guard !Task.isCancelled else { return }
            preview = p
            routes = RouteOptions.build(preview: p, sessions: model.sessions, context: voice.context)
            if let picked, !shown.options.contains(where: { $0.id == picked }) { self.picked = nil }
        } catch {
            if !Task.isCancelled { note = error.localizedDescription }
        }
    }

    private func send() async {
        guard let hub = model.hub, let chosen else { return }
        sending = true
        defer { sending = false }
        let text = voice.text.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            let r: Schemas.MessageResponse
            switch chosen.action {
            case .asPreviewed:
                r = try await hub.message(text, requestId: requestId, current: current)
            case .start(let project):
                r = try await hub.message(text, requestId: requestId, current: current, projectId: project)
            case .send(let target):
                r = try await hub.message(text, requestId: requestId, current: current, target: target)
            }
            note = r.message
            switch r.outcome {
            case "started", "sent", "selected":
                // Only `use <x>` makes a default; a send doesn't, so the home screen asks again.
                if r.outcome == "selected" { model.current = r.current }
                model.record(sent: text, to: r.current, started: r.outcome == "started")
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                voice.finish()
            case "choose_project", "choose_target":
                // The hub still couldn't tell; its options are the destinations now.
                let asked = Schemas.MessagePreview(outcome: r.outcome, message: r.message, options: r.options)
                preview = asked
                routes = RouteOptions.build(preview: asked, sessions: model.sessions, context: voice.context)
                picked = nil
            default:
                break
            }
        } catch let e as HubError where e.outcomeUnknown {
            await reconcile(hub, text: text)
        } catch {
            note = error.localizedDescription
        }
    }

    /// A lost answer: the hub may have acted, so look the request up before calling it failed.
    private func reconcile(_ hub: Hub, text: String) async {
        note = "Not sure that got through. Checking…"
        var state = StartState.unknown(requestId: requestId)
        for attempt in 0..<30 {
            state = await StartFlow.reconcile(hub: hub, requestId: requestId, attempt: attempt)
            guard case .unknown = state, !Task.isCancelled else { break }
            try? await Task.sleep(for: .seconds(2))
        }
        switch state {
        case .started(let sessionId, _):
            if let s = try? await hub.session(sessionId) {
                model.record(sent: text, to: .session(s), started: true)
            }
            voice.finish()
        case .unknown:
            note = "Still not sure it went through. Sending it again is safe."
        case .failed:
            note = "That didn't reach the hub. Send it again."
        }
    }
}

/// The last row under "Goes to": a menu of projects to start a new task in.
private struct NewTaskRow: View {
    let projects: [String]
    let pick: (String) -> Void

    var body: some View {
        Menu {
            ForEach(projects, id: \.self) { p in Button(p) { pick(p) } }
        } label: {
            RadioRow(title: "New task in…", on: false) { Note(text: "Pick a project") }
        }
        .foregroundStyle(Ink.ink)
        .accessibilityIdentifier("routeNewTask")
    }
}

private struct RouteRow: View {
    let option: RouteOption
    let selected: Bool
    let first: Bool
    let pick: () -> Void

    var body: some View {
        Button(action: pick) {
            RadioRow(title: option.title, on: selected, first: first) {
                Text(option.detail).font(Face.sub).foregroundStyle(Ink.sub).lineLimit(1)
            }
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("route-\(option.id)")
    }
}
