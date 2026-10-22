import SwiftUI
import SwitchboardKit

/// Everything voice mode draws over the app: the edge glow and the listening panel.
struct VoiceOverlay: View {
    @Environment(Voice.self) private var voice

    var body: some View {
        ZStack(alignment: .bottom) {
            if voice.phase == .listening {
                Color.clear
                    .contentShape(Rectangle())
                    .onTapGesture { voice.cancel() }
                    .accessibilityHidden(true)
                ListeningPanel()
                    .padding(10)
                    .transition(.materialize)
            }
            EdgeGlow()
        }
        .ignoresSafeArea()
        .animation(.spring(response: 0.38, dampingFraction: 1), value: voice.phase == .listening)
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

/// What's being heard, with Done to stop listening, ✕ to cancel and the keyboard to type instead.
struct ListeningPanel: View {
    @Environment(Voice.self) private var voice
    @Environment(\.accessibilityReduceTransparency) private var solid

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(voice.context.map { "To \($0.label)" } ?? "Switchboard picks the agent")
                .font(.footnote.weight(.semibold))
                .foregroundStyle(.secondary)
            transcript
                .font(.title2.weight(.semibold))
                .frame(maxWidth: .infinity, minHeight: 60, alignment: .topLeading)
                .accessibilityIdentifier("transcript")
            HStack {
                CircleButton(symbol: "xmark", label: "Cancel") { voice.cancel() }
                Spacer()
                Button { voice.stop() } label: {
                    HStack(spacing: 10) {
                        LevelBars(level: voice.level)
                        Text("Done")
                    }
                    .font(.headline)
                    .padding(.leading, 16)
                    .padding(.trailing, 20)
                    .frame(height: 44)
                    .background(Color(white: 0.11), in: Capsule())
                    .foregroundStyle(.white)
                }
                .accessibilityIdentifier("voiceDone")
                Spacer()
                CircleButton(symbol: "keyboard", label: "Type instead") { voice.typeInstead() }
            }
        }
        .padding(EdgeInsets(top: 20, leading: 20, bottom: 16, trailing: 20))
        .background {
            RoundedRectangle(cornerRadius: 38, style: .continuous)
                .fill(solid ? AnyShapeStyle(Color(.systemBackground)) : AnyShapeStyle(.regularMaterial))
                .shadow(color: .black.opacity(0.12), radius: 20, y: 12)
        }
    }

    /// Finished words in full colour, the one still being recognised in grey.
    private var transcript: Text {
        let words = voice.heard.split(separator: " ")
        guard let last = words.last else {
            return Text(voice.text.isEmpty ? "Listening…" : voice.text).foregroundColor(voice.text.isEmpty ? .secondary : .primary)
        }
        let done = ([voice.text] + words.dropLast().map(String.init)).filter { !$0.isEmpty }.joined(separator: " ")
        return Text(done.isEmpty ? "" : done + " ") + Text(String(last)).foregroundColor(.secondary)
    }
}

struct LevelBars: View {
    let level: Float
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 15, paused: reduceMotion)) { ctx in
            let t = ctx.date.timeIntervalSinceReferenceDate
            HStack(spacing: 3) {
                ForEach(0..<5, id: \.self) { i in
                    let wave = 0.55 + 0.45 * abs(sin(t * 9 + Double(i) * 1.7))
                    let edge = i == 0 || i == 4 ? 0.6 : 1.0
                    Capsule()
                        .fill(LinearGradient(colors: [Color(red: 0.55, green: 0.36, blue: 0.96), Color(red: 1, green: 0.3, blue: 0.55)], startPoint: .top, endPoint: .bottom))
                        .frame(width: 3, height: 4 + 16 * min(1, Double(level) * wave * edge))
                }
            }
            .frame(height: 20)
            .animation(.linear(duration: 0.08), value: level)
        }
        .accessibilityHidden(true)
    }
}

struct CircleButton: View {
    let symbol: String
    let label: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 17, weight: .semibold))
                .frame(width: 44, height: 44)
                .background(Color(.systemGray5), in: Circle())
                .foregroundStyle(Color(.label))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
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
