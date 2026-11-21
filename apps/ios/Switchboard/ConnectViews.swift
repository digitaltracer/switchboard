import SwiftUI
import SwitchboardKit

/// What home says before the hub's first answer: connecting, or which step failed and why.
struct ConnectState {
    struct Step {
        enum Kind { case ok, failed, unreached }
        let name: String
        let detail: String
        let kind: Kind
    }

    let headline: Headline
    let explanation: String?
    let steps: [Step]

    @MainActor
    init(_ model: AppModel, slow: Bool) {
        let relay = model.relayHost ?? "relay"
        guard model.unreachable else {
            headline = Headline("Connecting to your hub.", slow ? "This is taking a while. A weak signal slows it down." : "Your agents show here once it answers.")
            explanation = nil
            steps = []
            return
        }
        let phone = Step(name: "This phone", detail: "Online", kind: .ok)
        switch (model.reach, model.viaRelay) {
        case (.offline, _):
            headline = Headline("This phone is offline.", "Still trying.")
            explanation = "Join Wi-Fi or turn on mobile data. Switchboard reconnects by itself, and your agents keep working meanwhile."
            steps = [Step(name: "This phone", detail: "No network", kind: .failed)]
                + (model.viaRelay ? [Step(name: "Relay", detail: relay, kind: .unreached)] : [])
                + [Step(name: "Your hub", detail: "", kind: .unreached)]
        case (.hub, _):
            headline = Headline("Your hub is offline.", "Still trying.")
            explanation = "The relay is up, but your hub isn't connected to it. Check that the hub's machine is awake and the hub is running."
            steps = [phone, Step(name: "Relay", detail: relay, kind: .ok), Step(name: "Your hub", detail: "Not connected", kind: .failed)]
        case (.slow, _):
            headline = Headline("Your hub isn't answering.", "Still trying.")
            explanation = "It's connected but slow to reply, usually because its machine is busy or on a weak network. It tends to catch up."
            steps = [phone, Step(name: "Relay", detail: relay, kind: .ok), Step(name: "Your hub", detail: "Slow to answer", kind: .failed)]
        case (_, true):
            headline = Headline("Can’t reach the relay.", "Still trying.")
            explanation = "A weak signal can do this, or the relay may be down for a moment. Your agents keep working on your hub meanwhile."
            steps = [phone, Step(name: "Relay", detail: "Not answering", kind: .failed), Step(name: "Your hub", detail: "", kind: .unreached)]
        case (_, false):
            headline = Headline("Can’t reach your hub.", "Still trying.")
            explanation = "Check this phone's connection, and that the hub's machine is on and the hub is running."
            steps = [phone, Step(name: "Your hub", detail: "Not answering", kind: .failed)]
        }
    }
}

/// In place of the lists while the hub can't be reached: the path, what to check, and Try again.
struct HubUnreachable: View {
    @Environment(AppModel.self) private var model
    let state: ConnectState

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ReachPath(steps: state.steps)
                .padding(.top, 30)
            if let explanation = state.explanation {
                Text(explanation)
                    .font(Face.geist(16, relativeTo: .body))
                    .foregroundStyle(Ink.sub)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 30)
            }
            Spacer(minLength: 24)
            Button(model.trying ? "Trying…" : "Try again") { model.startPolling() }
                .buttonStyle(BigButtonStyle(kind: .outline))
                .disabled(model.trying)
                .accessibilityIdentifier("tryAgain")
            Text("Switchboard also tries again by itself every few seconds.")
                .font(Face.meta)
                .foregroundStyle(Ink.mut)
                .frame(maxWidth: .infinity)
                .padding(.top, 10)
        }
        .padding(.horizontal, 20)
        .padding(.bottom, 16)
        .accessibilityIdentifier("hubUnreachable")
    }
}

/// Phone → relay → hub as dots on a line: green up to the step that failed, orange there, grey after.
/// A pulse travels from the phone and stops at the break.
struct ReachPath: View {
    let steps: [ConnectState.Step]
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(spacing: 12) {
            TimelineView(.animation(paused: reduceMotion)) { t in
                Canvas { ctx, size in draw(ctx, size, t.date) }
            }
            .frame(height: 22)
            HStack(alignment: .top, spacing: 8) {
                ForEach(steps.indices, id: \.self) { i in
                    let s = steps[i]
                    let align: HorizontalAlignment = i == 0 ? .leading : i == steps.count - 1 ? .trailing : .center
                    VStack(alignment: align, spacing: 2) {
                        Text(s.name)
                            .font(Face.geist(15, .medium, relativeTo: .subheadline))
                            .foregroundStyle(s.kind == .unreached ? Ink.mut : Ink.ink)
                        Text(s.detail.isEmpty ? " " : s.detail)
                            .font(Face.meta)
                            .foregroundStyle(s.kind == .failed ? Ink.warn : Ink.mut)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    .frame(maxWidth: .infinity, alignment: Alignment(horizontal: align, vertical: .top))
                }
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(steps.map { "\($0.name): \($0.detail)" }.joined(separator: ", "))
    }

    private func x(_ i: Int, _ w: CGFloat) -> CGFloat {
        let r: CGFloat = 12
        return steps.count < 2 ? r : r + (w - 2 * r) * CGFloat(i) / CGFloat(steps.count - 1)
    }

    private func draw(_ ctx: GraphicsContext, _ size: CGSize, _ now: Date) {
        let y = size.height / 2
        let failed = steps.firstIndex { $0.kind == .failed } ?? steps.count - 1
        for i in 0..<(steps.count - 1) {
            var line = Path()
            line.move(to: CGPoint(x: x(i, size.width), y: y))
            line.addLine(to: CGPoint(x: x(i + 1, size.width), y: y))
            let reached = i + 1 < failed
            let broken = i + 1 == failed
            ctx.stroke(line, with: .color(reached ? Ink.ok.opacity(0.55) : broken ? Ink.warn.opacity(0.7) : Ink.line),
                       style: StrokeStyle(lineWidth: 2, lineCap: .round, dash: reached ? [] : [3, 5]))
        }
        // The pulse runs from the phone to the failed step, then starts over.
        if failed > 0, !reduceMotion {
            let p = now.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: 2.2) / 2.2
            let px = x(0, size.width) + (x(failed, size.width) - x(0, size.width)) * min(p / 0.8, 1)
            let fade = p < 0.8 ? 1 : 1 - (p - 0.8) / 0.2
            ctx.fill(Path(ellipseIn: CGRect(x: px - 4, y: y - 4, width: 8, height: 8)), with: .color(Ink.ok.opacity(0.9 * fade)))
        }
        for (i, s) in steps.enumerated() {
            let c = CGPoint(x: x(i, size.width), y: y)
            let dot = Path(ellipseIn: CGRect(x: c.x - 7, y: c.y - 7, width: 14, height: 14))
            switch s.kind {
            case .ok: ctx.fill(dot, with: .color(Ink.ok))
            case .failed:
                let beat = reduceMotion ? 0 : (sin(now.timeIntervalSinceReferenceDate * 3) + 1) / 2
                let halo = Path(ellipseIn: CGRect(x: c.x - 11, y: c.y - 11, width: 22, height: 22))
                ctx.fill(halo, with: .color(Ink.warn.opacity(0.12 + 0.14 * beat)))
                ctx.fill(dot, with: .color(Ink.warn))
            case .unreached:
                ctx.fill(dot, with: .color(Ink.bg))
                ctx.stroke(dot, with: .color(Ink.line), lineWidth: 2)
            }
        }
    }
}
