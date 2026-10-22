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
    @State private var note: String?
    @State private var sending = false
    /// One per draft and destination, so a retried send is never typed twice.
    @State private var requestId = UUID().uuidString

    private var current: Schemas.TargetView? { voice.context ?? model.current }
    private var chosen: RouteOption? { routes.options.first { $0.id == (picked ?? routes.suggested) } }
    private var empty: Bool { voice.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var body: some View {
        @Bindable var voice = voice
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if let error = voice.error {
                    Label(error, systemImage: "mic.slash").font(.footnote).foregroundStyle(.secondary)
                }
                TextField(current.map { "Message \($0.label)" } ?? "Say or type what you want done", text: $voice.text, axis: .vertical)
                    .font(.title3)
                    .lineLimit(2...6)
                    .padding(14)
                    .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                    .focused($focused)
                    .accessibilityIdentifier("composeField")
                if !routes.options.isEmpty {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(preview?.message ?? "Goes to")
                            .font(.footnote.weight(.semibold))
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 4)
                            .accessibilityIdentifier("previewMessage")
                        VStack(spacing: 0) {
                            ForEach(Array(routes.options.enumerated()), id: \.element.id) { i, o in
                                RouteRow(option: o, selected: o.id == chosen?.id, first: i == 0) {
                                    picked = o.id
                                    requestId = UUID().uuidString
                                }
                            }
                        }
                        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                    }
                } else if let preview, preview.outcome == "refused" {
                    Text(preview.message).font(.footnote).foregroundStyle(.secondary)
                }
                if let note {
                    Text(note).font(.footnote).foregroundStyle(.secondary).accessibilityIdentifier("composeNote")
                }
            }
            .padding(.horizontal, 20)
        }
        .scrollDismissesKeyboard(.interactively)
        .safeAreaInset(edge: .top, spacing: 12) {
            HStack {
                Button("Cancel") { voice.finish() }
                Spacer()
                Text("Ready to send").font(.headline)
                Spacer()
                Button("Cancel") {}.hidden()
            }
            .padding(.horizontal, 20)
            .padding(.top, 14)
        }
        .safeAreaInset(edge: .bottom) {
            HStack(spacing: 10) {
                Button { voice.listen(to: voice.context, keepText: true) } label: {
                    Image(systemName: "mic.fill")
                        .font(.system(size: 20, weight: .semibold))
                        .frame(width: 50, height: 50)
                        .background(Color(.tertiarySystemFill), in: Circle())
                }
                .accessibilityLabel("Keep talking")
                Button { Task { await send() } } label: {
                    Group {
                        if sending { ProgressView().tint(.white) } else { Text(chosen?.verb ?? (empty ? "Say or type something" : "Pick where this goes")) }
                    }
                    .font(.headline)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, minHeight: 50)
                    .background(chosen == nil || empty ? Color.gray : Color.accentColor, in: Capsule())
                    .foregroundStyle(.white)
                }
                .disabled(chosen == nil || empty || sending)
                .accessibilityIdentifier("composeSend")
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 8)
            .background(Color(.systemGroupedBackground))
        }
        .background(Color(.systemGroupedBackground))
        .presentationDetents([.medium, .large])
        .presentationCornerRadius(28)
        .onAppear { if voice.text.isEmpty { focused = true } }
        .task(id: voice.text) { await loadPreview() }
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
            if let picked, !routes.options.contains(where: { $0.id == picked }) { self.picked = nil }
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
                model.current = r.current
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
                model.current = .session(s)
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

private struct RouteRow: View {
    let option: RouteOption
    let selected: Bool
    let first: Bool
    let pick: () -> Void

    var body: some View {
        Button(action: pick) {
            HStack(spacing: 12) {
                Image(systemName: option.isNew ? "plus" : "waveform.path.ecg")
                    .font(.system(size: 15, weight: .semibold))
                    .frame(width: 32, height: 32)
                    .background(option.isNew ? Color(.tertiarySystemFill) : Color.accentColor.opacity(0.14), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                    .foregroundStyle(option.isNew ? Color.primary : Color.accentColor)
                VStack(alignment: .leading, spacing: 1) {
                    Text(option.title).font(.body.weight(.semibold)).lineLimit(1)
                    Text(option.detail).font(.footnote).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 0)
                Image(systemName: "checkmark")
                    .font(.system(size: 15, weight: .bold))
                    .foregroundStyle(Color.accentColor)
                    .opacity(selected ? 1 : 0)
            }
            .padding(.horizontal, 14)
            .frame(minHeight: 56)
            .background(selected ? Color.accentColor.opacity(0.06) : .clear)
            .overlay(alignment: .top) {
                if !first { Divider().padding(.leading, 58) }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .accessibilityIdentifier("route-\(option.id)")
    }
}
