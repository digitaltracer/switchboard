import SwiftUI
import SwitchboardLiveKit

enum LiveIsland {
    static func glyph(_ mood: LiveMood, _ board: LiveBoard?) -> String {
        switch mood {
        case .needsYou: "live-sun"
        case .error: "live-sun-red"
        case .finished: "live-finished"
        case .calm: (board?.runningCount ?? 0) == 0 ? "live-moon" : "live-jupiter"
        }
    }

    /// The mini system, then the name of the agent the card is about.
    struct System: View {
        let board: LiveBoard?
        let mood: LiveMood

        var body: some View {
            HStack(spacing: 4) {
                LiveGlyph(image: LiveIsland.glyph(mood, board), size: 16)
                ForEach(Array((board?.running ?? []).prefix(2).enumerated()), id: \.offset) { i, name in
                    LiveGlyph(image: "live-\(LiveLayout.texture(name))", size: i == 0 ? 9 : 7)
                }
                if (board?.idle ?? 0) > 0 { LiveGlyph(image: "live-moon", size: 4) }
                if let name {
                    Text(name)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(.white)
                        .lineLimit(1)
                        .frame(maxWidth: 76, alignment: .leading)
                        .padding(.leading, 3)
                }
            }
        }

        private var name: String? {
            switch mood {
            case .needsYou(let w, _), .error(let w, _): LiveLayout.short(w.name)
            case .finished(let n): LiveLayout.short(n)
            case .calm: nil
            }
        }
    }

    struct Trailing: View {
        let board: LiveBoard?
        let mood: LiveMood

        var body: some View {
            switch mood {
            case .needsYou(let w, _):
                LiveClock(since: w.since, width: 52, alignment: .trailing)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(LiveCard.amber)
            case .error:
                Text("error").font(.system(size: 14, weight: .semibold)).foregroundStyle(LiveCard.red)
            case .finished:
                Text("done").font(.system(size: 14, weight: .semibold)).foregroundStyle(LiveCard.green)
            case .calm:
                Text("\(board?.runningCount ?? 0) running")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(LiveCard.counts)
            }
        }
    }
}
