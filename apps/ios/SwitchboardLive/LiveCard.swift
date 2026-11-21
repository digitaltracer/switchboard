import SwiftUI
import SwitchboardLiveKit

/// The lock-screen card: the system on the left, three lines on the right. Every state uses the
/// same lines: a name, what happened, and the counts.
struct LiveCard: View {
    let board: LiveBoard
    let stale: Bool

    static let ground = Color(red: 0.020, green: 0.027, blue: 0.039)
    static let amber = Color(red: 1, green: 0.702, blue: 0.498)
    static let red = Color(red: 1, green: 0.604, blue: 0.565)
    static let green = Color(red: 0.435, green: 0.827, blue: 0.627)
    static let ink = Color(red: 0.95, green: 0.953, blue: 0.957)
    static let muted = Color(red: 0.663, green: 0.682, blue: 0.706)
    static let counts = Color(red: 0.725, green: 0.745, blue: 0.769)
    static let textX: CGFloat = 184

    /// Changes when the card's subject does, which is when the lead ignites.
    static func key(_ b: LiveBoard, _ m: LiveMood) -> String {
        switch m {
        case .calm: "calm"
        case .needsYou(let w, let n): "needs|\(w.name)|\(n > 0)"
        case .error(let w, _): "error|\(w.name)"
        case .finished(let n): "done|\(n)"
        }
    }

    var body: some View {
        let mood = LiveMood(board)
        ZStack(alignment: .topLeading) {
            LiveScene(board: board, mood: mood, fade: Self.textX)
            LiveLines(board: board, mood: mood)
                .padding(.leading, Self.textX)
                .padding(.trailing, 14)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
        }
        .frame(height: 150)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(alignment: .leading) {
            Image("live-stars").resizable().frame(width: 420, height: 150)
        }
        .clipped()
        .grayscale(stale ? 1 : 0)
        .opacity(stale ? 0.55 : 1)
        .animation(.spring(duration: 1.8, bounce: 0.3), value: Self.key(board, mood))
    }
}

/// A small body before a line, the same shape as on the system.
struct LiveGlyph: View {
    let image: String
    var size: CGFloat = 15

    var body: some View {
        Image(image).resizable().frame(width: size * 2, height: size * 2).frame(width: size, height: size)
    }
}

struct LiveLines: View {
    let board: LiveBoard
    let mood: LiveMood
    var big: CGFloat = 24

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 8) {
                if let leadGlyph { LiveGlyph(image: leadGlyph) }
                Text(title)
                    .font(.system(size: big, weight: .bold))
                    .foregroundStyle(LiveCard.ink)
                    .lineLimit(1)
                    .minimumScaleFactor(0.6)
            }
            happened
                .font(.system(size: big * 0.64, weight: .medium))
                .lineLimit(1)
                .minimumScaleFactor(0.75)
                .padding(.leading, leadGlyph == nil ? 0 : 23)
            HStack(spacing: 14) {
                if case .needsYou(_, let n) = mood, n > 0 {
                    count("live-sun", Text("+\(n) more").foregroundStyle(LiveCard.amber))
                    count(planet, Text("\(board.runningCount)"))
                    count("live-moon", Text("\(board.idle)"))
                } else if case .calm = mood {
                    count("live-moon", Text("\(board.idle) idle"))
                } else {
                    count(planet, Text("\(board.runningCount) running"))
                    count("live-moon", Text("\(board.idle) idle"))
                }
            }
            .font(.system(size: big * 0.54, weight: .medium))
            .foregroundStyle(LiveCard.counts)
        }
    }

    private var planet: String { "live-jupiter" }

    /// None beside "All quiet": nothing is running, so no planet.
    private var leadGlyph: String? {
        switch mood {
        case .needsYou: "live-sun"
        case .error: "live-sun-red"
        case .finished: "live-finished"
        case .calm: board.runningCount == 0 ? nil : planet
        }
    }

    private var title: String {
        switch mood {
        case .needsYou(let w, _), .error(let w, _): w.name
        case .finished(let n): n
        case .calm: board.runningCount == 0 ? "All quiet" : "\(board.runningCount) running"
        }
    }

    @ViewBuilder private var happened: some View {
        switch mood {
        case .needsYou(let w, _):
            HStack(spacing: 0) {
                Text("needs you · ")
                LiveClock(since: w.since, width: big * 3.2)
            }
            .foregroundStyle(LiveCard.amber)
        case .error:
            Text("stopped on an error").foregroundStyle(LiveCard.red)
        case .finished:
            Text("finished just now").foregroundStyle(LiveCard.green)
        case .calm:
            Text("nothing needs you").foregroundStyle(LiveCard.muted)
        }
    }

    private func count(_ image: String, _ text: Text) -> some View {
        HStack(spacing: 6) {
            LiveGlyph(image: image, size: image == "live-moon" ? 8 : 12)
            text
        }
    }
}

/// A ticking wait clock in a fixed width, so it never truncates to "…". Never `.fixedSize()` on
/// it or around it: on a phone, a Live Activity with a fixed-size timer isn't drawn at all.
struct LiveClock: View {
    let since: Date
    var width: CGFloat = 46
    var alignment: Alignment = .leading

    var body: some View {
        Text(timerInterval: since...since.addingTimeInterval(86_400), countsDown: false)
            .monospacedDigit()
            .lineLimit(1)
            .multilineTextAlignment(alignment == .trailing ? .trailing : .leading)
            .frame(width: width, alignment: alignment)
    }
}

/// What shows when the state can't be opened: nothing about the agents.
struct LiveUnreadable: View {
    /// Debug builds say why, for diagnosing on a device.
    var detail: String?

    var body: some View {
        HStack(spacing: 12) {
            LiveGlyph(image: "live-sun", size: 22)
            VStack(alignment: .leading, spacing: 4) {
                Text("Open Switchboard to see your agents")
                    .font(.system(size: 15, weight: .medium))
                    .foregroundStyle(LiveCard.ink)
                if let detail {
                    Text(detail).font(.system(size: 11, design: .monospaced)).foregroundStyle(LiveCard.muted)
                }
            }
        }
        .padding(20)
        .frame(maxWidth: .infinity, minHeight: 90, alignment: .leading)
    }
}
