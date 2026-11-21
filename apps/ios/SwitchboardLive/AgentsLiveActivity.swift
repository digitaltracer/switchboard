import ActivityKit
import SwiftUI
import SwitchboardLiveKit
import WidgetKit

struct AgentsLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: AgentsActivity.self) { context in
            Group {
                switch Opener.open(context.state) {
                case .board(let b): LiveCard(board: b, stale: context.isStale)
                case let other: LiveUnreadable(detail: Opener.debugDetail(other))
                }
            }
            .activityBackgroundTint(LiveCard.ground)
            .activitySystemActionForegroundColor(.white)
        } dynamicIsland: { context in
            let board = Opener.board(context.state)
            let mood = board.map { LiveMood($0) } ?? .calm
            return DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    LiveGlyph(image: LiveIsland.glyph(mood, board), size: 34).padding(.leading, 8).padding(.top, 8)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    if let board {
                        LiveLines(board: board, mood: mood, big: 19)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.leading, 6)
                            .grayscale(context.isStale ? 1 : 0)
                    } else {
                        LiveUnreadable()
                    }
                }
            } compactLeading: {
                LiveIsland.System(board: board, mood: mood)
            } compactTrailing: {
                LiveIsland.Trailing(board: board, mood: mood)
            } minimal: {
                LiveGlyph(image: LiveIsland.glyph(mood, board), size: 18)
            }
            .keylineTint(LiveLead(mood).glow == .clear ? LiveCard.muted : LiveLead(mood).glow)
        }
    }
}

