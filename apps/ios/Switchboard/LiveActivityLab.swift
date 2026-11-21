import ActivityKit
import SwiftUI
import SwitchboardKit

/// Settings' lock-screen section: the card as your hub's board draws it now, and when it shows.
struct LiveActivityLab: View {
    @Environment(AppModel.self) private var model
    @State private var board: LiveBoard?
    @AppStorage(LiveShowMode.key) private var mode = LiveShowMode.working.rawValue

    var body: some View {
        SectionHead("Lock screen")
        if let board {
            LiveCardPreview(board: board).padding(.bottom, 8)
        }
        Text("Show on the lock screen").font(Face.geist(15, .medium)).foregroundStyle(Ink.ink).padding(.bottom, 8)
        HStack(spacing: 8) {
            ForEach([(LiveShowMode.working, "While agents are working"), (.always, "Always")], id: \.0) { m, label in
                Button {
                    mode = m.rawValue
                    Task {
                        await model.live.register()
                        await model.live.refresh()
                    }
                } label: {
                    Text(label)
                        .font(Face.geist(14, .medium))
                        .foregroundStyle(mode == m.rawValue ? Ink.inv : Ink.sub)
                        .frame(maxWidth: .infinity, minHeight: 44)
                        .background(mode == m.rawValue ? Ink.ink : Ink.fill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(mode == m.rawValue ? .isSelected : [])
                .accessibilityIdentifier("liveMode-\(m.rawValue)")
            }
        }
        Note(text: mode == LiveShowMode.always.rawValue
             ? "The card stays, with All quiet when nothing runs. iOS ends a card after eight hours, so your hub starts a fresh one before then; it may disappear briefly. Updates while the app is closed need your relay to be up to date."
             : "The card starts when an agent is working or needs you, and ends 15 minutes after everything is idle or done.")
            .padding(.top, 8)
        Note(text: "Your hub sends it encrypted to this phone. Turn it off in iOS Settings › Switchboard › Live Activities.")
            .padding(.top, 4)
            .task { board = try? await model.hub?.live() }
    }
}

/// The lock-screen card as the extension draws it, for Settings and screenshots.
struct LiveCardPreview: View {
    let board: LiveBoard
    var stale = false

    var body: some View {
        LiveCard(board: board, stale: stale)
            .background(LiveCard.ground)
            .clipShape(RoundedRectangle(cornerRadius: 24, style: .continuous))
            .environment(\.colorScheme, .dark)
    }
}

/// `SB_LIVE_PREVIEW=1`, or a list such as `error,stale`: the card's states on one screen, for
/// comparing with the design.
struct LivePreviewScreen: View {
    /// Debug builds only.
    static var requested: Bool {
        #if DEBUG
        ProcessInfo.processInfo.environment["SB_LIVE_PREVIEW"] != nil
        #else
        false
        #endif
    }

    private var names: [String] {
        let v = ProcessInfo.processInfo.environment["SB_LIVE_PREVIEW"] ?? "1"
        return v == "1" ? ["needs", "calm", "two", "finished", "error", "stale"] : v.split(separator: ",").map(String.init)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(names, id: \.self) { name in
                    Text(name.uppercased()).font(.system(size: 11, weight: .semibold)).foregroundStyle(.secondary)
                    if name == "island" {
                        IslandPreview()
                    } else {
                        LiveCardPreview(board: LiveDemo.board(name == "stale" ? "needs" : name), stale: name == "stale")
                    }
                }
            }
            .padding(14)
        }
        .background(Color(red: 0.11, green: 0.125, blue: 0.157))
    }
}

/// The Dynamic Island's compact, minimal and expanded views on black, as iOS frames them.
struct IslandPreview: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(["needs", "error", "finished", "calm"], id: \.self) { name in
                let b = LiveDemo.board(name)
                HStack(spacing: 12) {
                    HStack {
                        LiveIsland.System(board: b, mood: LiveMood(b))
                        Spacer(minLength: 40)
                        LiveIsland.Trailing(board: b, mood: LiveMood(b))
                    }
                    .padding(.horizontal, 14)
                    .frame(width: 300, height: 37)
                    .background(.black, in: Capsule())
                    LiveGlyph(image: LiveIsland.glyph(LiveMood(b), b), size: 18)
                        .frame(width: 37, height: 37)
                        .background(.black, in: Circle())
                }
            }
            let b = LiveDemo.board("two")
            HStack(alignment: .top) {
                LiveGlyph(image: LiveIsland.glyph(LiveMood(b), b), size: 34).padding(8)
                LiveLines(board: b, mood: LiveMood(b), big: 19)
                Spacer(minLength: 0)
            }
            .padding(14)
            .frame(width: 370)
            .background(.black, in: RoundedRectangle(cornerRadius: 40, style: .continuous))
        }
        .environment(\.colorScheme, .dark)
    }
}

enum LiveDemo {
    static func board(_ name: String, now: Date = Date()) -> LiveBoard {
        switch name {
        case "needs": LiveBoard(at: now, waiting: [.init(name: "checkout", space: "atlas-web", since: now.addingTimeInterval(-134))],
                                running: ["search", "kraft-7c1e", "importer"], idle: 2)
        case "two": LiveBoard(at: now, waiting: [.init(name: "checkout", space: "atlas-web", since: now.addingTimeInterval(-400)),
                                                .init(name: "search", space: "atlas-web", since: now.addingTimeInterval(-65))],
                              running: ["kraft-7c1e", "importer"], idle: 2)
        case "error": LiveBoard(at: now, waiting: [.init(name: "migrations", space: "infra", since: now.addingTimeInterval(-40), error: true)],
                                running: ["search", "checkout", "kraft-7c1e"], idle: 2)
        case "finished": LiveBoard(at: now, running: ["search", "checkout", "importer", "api"], idle: 1, finished: "kraft-7c1e", finishedAt: now)
        default: LiveBoard(at: now, running: ["checkout", "search", "importer", "kraft-7c1e", "api"], idle: 1)
        }
    }

    static func show(_ b: LiveBoard) async throws {
        let key = try LiveKey.shared.loadOrCreate()
        let state = AgentsActivity.ContentState(b: try LiveSeal.seal(try JSONEncoder().encode(b), to: key.publicKey))
        #if DEBUG
        // `SB_LIVE_DEMO_STALE=90` makes the card go stale then, which makes iOS draw it again.
        let stale = ProcessInfo.processInfo.environment["SB_LIVE_DEMO_STALE"].flatMap(Double.init) ?? 15 * 60
        #else
        let stale: Double = 15 * 60
        #endif
        let content = ActivityContent(state: state, staleDate: Date().addingTimeInterval(stale))
        if let a = Activity<AgentsActivity>.activities.first {
            await a.update(content)
        } else {
            _ = try Activity.request(attributes: AgentsActivity(), content: content)
        }
    }

    static func endAll() async {
        for a in Activity<AgentsActivity>.activities { await a.end(nil, dismissalPolicy: .immediate) }
    }

    /// `SB_LIVE_DEMO=calm|needs|two|error|finished` shows a demo card at launch, for screenshots;
    /// `SB_LIVE_DEMO_LATER=<name>` then changes it 30 s later from a background task.
    @MainActor
    static func fromEnvironment() async {
        #if DEBUG
        let env = ProcessInfo.processInfo.environment
        guard let name = env["SB_LIVE_DEMO"] else { return }
        await endAll()
        do {
            try await show(board(name))
            NSLog("SBLIVE shown %@", name)
        } catch {
            NSLog("SBLIVE failed %@", String(describing: error))
        }
        guard let later = env["SB_LIVE_DEMO_LATER"] else { return }
        let task = UIApplication.shared.beginBackgroundTask(withName: "live-demo")
        try? await Task.sleep(for: .seconds(30))
        try? await show(board(later))
        NSLog("SBLIVE shown later %@", later)
        UIApplication.shared.endBackgroundTask(task)
        #endif
    }
}

