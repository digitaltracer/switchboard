import ActivityKit
import CryptoKit
import SwiftUI
import SwitchboardLiveKit
import WidgetKit
import os

@main
struct SwitchboardLiveBundle: WidgetBundle {
    var body: some Widget { AgentsLiveActivity() }
}

/// Opens each state with the live key from the App Group container, read on every draw: it is
/// 32 bytes, and a draw while locked must really read it.
enum Opener {
    enum Result { case board(LiveBoard), noKey(String), sealed }

    static let log = Logger(subsystem: "switchboard.live", category: "open")

    static func board(_ s: AgentsActivity.ContentState) -> LiveBoard? {
        if case .board(let b) = open(s) { return b }
        return nil
    }

    static func open(_ s: AgentsActivity.ContentState) -> Result {
        let r = read(s)
        switch r {
        case .board(let b): log.info("SBLIVE opened waiting=\(b.waitingCount) running=\(b.runningCount)")
        case .noKey(let why): log.error("SBLIVE no key: \(why, privacy: .public)")
        case .sealed: log.error("SBLIVE did not open")
        }
        return r
    }

    /// Why a state didn't open, shown on the card in debug builds only.
    static func debugDetail(_ r: Result) -> String? {
        #if DEBUG
        switch r {
        case .board: return nil
        case .noKey(let why): return "no key: \(why) · group \(LiveKey.shared.groupName ?? "none")"
        case .sealed: return "doesn't open"
        }
        #else
        return nil
        #endif
    }

    private static func read(_ s: AgentsActivity.ContentState) -> Result {
        let key: P256.KeyAgreement.PrivateKey?
        do {
            key = try LiveKey.shared.load()
        } catch {
            return .noKey("\(error)")
        }
        guard let key else { return .noKey("no key") }
        guard let b = try? LiveSeal.board(s.b, with: key) else { return .sealed }
        return .board(b)
    }
}
