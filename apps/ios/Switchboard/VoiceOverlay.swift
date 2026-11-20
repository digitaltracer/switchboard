import SwiftUI
import SwitchboardKit

/// Everything voice mode draws over the app: the listening page and the edge glow.
struct VoiceOverlay: View {
    @Environment(Voice.self) private var voice

    var body: some View {
        let shown = voice.phase == .listening || voice.phase == .transcribing
        ZStack(alignment: .bottom) {
            if shown {
                ListeningPanel()
                    .transition(.materialize)
            }
            EdgeGlow().ignoresSafeArea()
        }
        .animation(.spring(response: 0.38, dampingFraction: 1), value: shown)
    }
}

/// The light along the screen's edge while listening, drawn by `edgeGlow` in EdgeGlow.metal.
struct EdgeGlow: View {
    @Environment(Voice.self) private var voice
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var start = Date()
    /// Still drawing the fade after listening stopped.
    @State private var lingering = false

    var body: some View {
        let listening = voice.phase == .listening
        Group {
            if listening || lingering { glow(listening: listening) }
        }
        .onChange(of: listening) { was, _ in
            guard was else { return }
            lingering = true
            Task { try? await Task.sleep(for: .milliseconds(500)); lingering = false }
        }
    }

    private func glow(listening: Bool) -> some View {
        let voice = voice
        let reduceMotion = reduceMotion
        let start = start
        return EdgeGlowView(cornerRadius: Float(Self.cornerRadius)) {
            let now = Date()
            let (reveal, fade) = Self.envelope(listening: voice.phase == .listening, t: now.timeIntervalSince(voice.changedAt), reduceMotion: reduceMotion)
            return GlowFrame(
                time: reduceMotion ? 2 : Float(now.timeIntervalSince(start)),
                level: reduceMotion ? 0.3 : voice.level,
                reveal: Float(reveal),
                fade: Float(fade)
            )
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    /// The sweep up the edges as listening starts, and the fade when it stops.
    static func envelope(listening: Bool, t: Double, reduceMotion: Bool) -> (reveal: Double, fade: Double) {
        let ease = { (x: Double) in 1 - pow(1 - min(max(x, 0), 1), 3) }
        if listening {
            return reduceMotion ? (1, min(t / 0.25, 1)) : (ease(t / 0.95), min(t / 0.12, 1))
        }
        let k = min(t / 0.45, 1)
        return (reduceMotion ? 1 : 1 - 0.35 * k * k, 1 - ease(k))
    }

    /// Close to the display's own corners on Face ID iPhones.
    static var cornerRadius: Double {
        let window = UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.keyWindow }.first
        return (window?.safeAreaInsets.bottom ?? 0) > 0 ? 52 : 0
    }
}

/// What's being heard, full screen: the words as they come, a level meter, then Cancel, Done and
/// Type instead.
struct ListeningPanel: View {
    @Environment(Voice.self) private var voice

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Circle().fill(Ink.bad).frame(width: 8, height: 8)
                Text(voice.cloud ? "Recording for OpenAI" : "Listening on this phone")
            }
            .font(Face.geist(13, .medium, relativeTo: .footnote))
            .foregroundStyle(Ink.sub)
            .frame(height: 44)
            Text((voice.dictating ? "Dictating the prompt" : voice.context.map { "To \($0.label)" } ?? "Switchboard picks the agent").uppercased())
                .font(Face.section)
                .tracking(0.84)
                .foregroundStyle(Ink.mut)
                .padding(.top, 36)
            transcript
                .font(Face.geist(38, .semibold, relativeTo: .largeTitle))
                .tracking(-1.3)
                .minimumScaleFactor(0.4)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .padding(.top, 8)
                .accessibilityIdentifier("transcript")
            if let notice = voice.notice {
                Note(text: notice).accessibilityIdentifier("voiceNotice").padding(.bottom, 12)
            }
            if voice.phase == .transcribing {
                HStack(spacing: 10) {
                    ProgressView()
                    Text("Transcribing with OpenAI…").font(Face.sub).foregroundStyle(Ink.sub)
                }
                .frame(maxWidth: .infinity)
                .padding(.bottom, 28)
                HStack {
                    Button { voice.cancel() } label: { RoundIcon(symbol: "xmark") }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Cancel")
                    Spacer()
                }
            } else {
                Wave(level: voice.level).frame(maxWidth: .infinity).padding(.bottom, 28)
                HStack(spacing: 12) {
                    Button { voice.cancel() } label: { RoundIcon(symbol: "xmark") }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Cancel")
                    Button { voice.stop() } label: { Text("Done") }
                        .buttonStyle(BigButtonStyle())
                        .clipShape(Capsule())
                        .accessibilityIdentifier("voiceDone")
                    Button { voice.typeInstead() } label: { RoundIcon(symbol: "keyboard") }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Type instead")
                }
            }
        }
        .padding(.horizontal, 20)
        .padding(.bottom, 12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Ink.sf.ignoresSafeArea())
    }

    /// Finished words in full colour, the one still being recognised quieter.
    private var transcript: Text {
        let words = voice.heard.split(separator: " ")
        if voice.cloud {
            let status = voice.phase == .transcribing ? "Transcribing with OpenAI…" : "Recording for OpenAI…"
            guard !voice.text.isEmpty else { return Text(status).foregroundColor(Ink.mut) }
            return Text(voice.text + "\n") + Text(status).foregroundColor(Ink.mut)
        }
        guard let last = words.last else {
            return Text(voice.text.isEmpty ? "Listening…" : voice.text).foregroundColor(voice.text.isEmpty ? Ink.mut : Ink.ink)
        }
        let done = ([voice.text] + words.dropLast().map(String.init)).filter { !$0.isEmpty }.joined(separator: " ")
        return Text(done.isEmpty ? "" : done + " ").foregroundColor(Ink.ink) + Text(String(last)).foregroundColor(Ink.mut)
    }
}

/// Bars that follow how loud it is.
struct Wave: View {
    let level: Float
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    private static let shape: [Double] = [0.2, 0.35, 0.65, 0.95, 0.55, 0.8, 1, 0.45, 0.7, 0.9, 0.4, 0.6, 0.25, 0.15]

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 15, paused: reduceMotion)) { ctx in
            let t = ctx.date.timeIntervalSinceReferenceDate
            HStack(spacing: 4) {
                ForEach(Self.shape.indices, id: \.self) { i in
                    let wave = 0.6 + 0.4 * abs(sin(t * 7 + Double(i) * 1.3))
                    Capsule()
                        .fill(Ink.ink)
                        .frame(width: 4, height: 6 + 34 * Self.shape[i] * min(1, 0.25 + Double(level) * 1.6) * wave)
                }
            }
            .frame(height: 40)
            .animation(.linear(duration: 0.08), value: level)
        }
        .accessibilityHidden(true)
    }
}

extension AnyTransition {
    /// Rises from where the mic was and comes into focus.
    static var materialize: AnyTransition {
        .modifier(active: Materialize(progress: 0), identity: Materialize(progress: 1))
    }
}

private struct Materialize: ViewModifier {
    var progress: Double
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        content
            .opacity(progress)
            .blur(radius: reduceMotion ? 0 : (1 - progress) * 8)
            .scaleEffect(reduceMotion ? 1 : 0.9 + 0.1 * progress, anchor: .bottom)
            .offset(y: reduceMotion ? 0 : (1 - progress) * 40)
    }
}
