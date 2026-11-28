import CryptoKit
import Foundation
import Testing

@testable import SwitchboardKit

func liveVector() throws -> [String: Any] {
    let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    let data = try Data(contentsOf: root.appending(path: "tests/fixtures/app-v1/live-vector.json"))
    return try JSONSerialization.jsonObject(with: data) as! [String: Any]
}

@Test func theHubsSealedBoardOpensWithTheLiveKey() throws {
    let v = try liveVector()
    let key = try P256.KeyAgreement.PrivateKey(rawRepresentation: hexData(v["device_secret_hex"] as! String))
    #expect(key.publicKey.x963Representation.base64URL == v["device_public"] as! String)
    let plain = try LiveSeal.open(v["blob"] as! String, with: key)
    #expect(String(decoding: plain, as: UTF8.self) == v["plain"] as! String)

    let board = try LiveSeal.board(v["blob"] as! String, with: key)
    #expect(board.waiting.count == 2)
    #expect(board.waiting[0].name.hasPrefix("agent-name-00"))
    #expect(board.waiting[1].error)
    #expect(board.waitingCount == 9)
    #expect(board.runningCount == 28)
    #expect(board.idle == 40)
    #expect(board.at == Date(timeIntervalSince1970: 1_700_000_000))

    #expect(throws: LiveSeal.Failure.self) { try LiveSeal.open(v["blob"] as! String, with: P256.KeyAgreement.PrivateKey()) }
    #expect(throws: LiveSeal.Failure.self) { try LiveSeal.open("AAAA", with: key) }
}

@Test func aBoardRoundTripsThroughItsShortKeys() throws {
    let b = LiveBoard(at: Date(timeIntervalSince1970: 100), waiting: [.init(name: "checkout", space: "atlas", since: Date(timeIntervalSince1970: 50))],
                      running: ["api", "docs"], idle: 2, finished: "kraft", finishedAt: Date(timeIntervalSince1970: 90))
    let json = try JSONEncoder().encode(b)
    #expect(try JSONDecoder().decode(LiveBoard.self, from: json) == b)
    #expect(String(decoding: json, as: UTF8.self).contains("\"a\":\"checkout\""))
}

@Test func theMoodFollowsTheDesignsStates() {
    let now = Date(timeIntervalSince1970: 1000)
    let w = LiveBoard.Waiting(name: "checkout", since: now)
    #expect(LiveMood(LiveBoard(running: ["a"], idle: 1), now: now) == .calm)
    #expect(LiveMood(LiveBoard(waiting: [w], more: 1), now: now) == .needsYou(w, others: 1))
    var e = w
    e.error = true
    #expect(LiveMood(LiveBoard(waiting: [e]), now: now) == .error(e, others: 0))
    #expect(LiveMood(LiveBoard(finished: "kraft", finishedAt: now.addingTimeInterval(-30)), now: now) == .finished("kraft"))
    #expect(LiveMood(LiveBoard(finished: "kraft", finishedAt: now.addingTimeInterval(-61)), now: now) == .calm)
    #expect(LiveMood(LiveBoard(waiting: [w]), now: now).urgent)
}

@Test func whatTheAppSealsTheExtensionOpens() throws {
    let key = P256.KeyAgreement.PrivateKey()
    let b = LiveBoard(at: Date(timeIntervalSince1970: 7), running: ["api"], idle: 3)
    let blob = try LiveSeal.seal(try JSONEncoder().encode(b), to: key.publicKey)
    #expect(try LiveSeal.board(blob, with: key) == b)
}

@Test func theLiveKeyIsAPrivateFileMadeOnceAndNeverBackedUp() throws {
    let dir = FileManager.default.temporaryDirectory.appending(path: "livekey-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = LiveKey(directory: dir)
    #expect(try store.load() == nil)
    let made = try store.loadOrCreate()
    #expect(try store.loadOrCreate().rawRepresentation == made.rawRepresentation, "made once")
    #expect(try LiveKey(directory: dir).load()?.rawRepresentation == made.rawRepresentation)
    let file = dir.appending(path: "live-key")
    let mode = try FileManager.default.attributesOfItem(atPath: file.path)[.posixPermissions] as? Int
    #expect(mode == 0o600)
    #expect(try file.resourceValues(forKeys: [.isExcludedFromBackupKey]).isExcludedFromBackup == true)
    #expect(throws: LiveKey.Failure.self) { try LiveKey(directory: nil).load() }
}

@Test func aCorruptKeyFileIsReplacedButAnUnreadableOneIsNot() throws {
    let dir = FileManager.default.temporaryDirectory.appending(path: "livekey-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: dir) }
    try Data([1, 2, 3]).write(to: dir.appending(path: "live-key"))
    #expect(throws: LiveKey.Failure.self) { try LiveKey(directory: dir).load() }
    let fresh = try LiveKey(directory: dir).loadOrCreate()
    #expect(try LiveKey(directory: dir).load()?.rawRepresentation == fresh.rawRepresentation)
}

@Test func whileWorkingTheCardStartsForWorkAndEndsAfterFifteenQuietMinutes() {
    let now = Date(timeIntervalSince1970: 10_000)
    let quiet = LiveBoard(idle: 9)
    let busy = LiveBoard(running: ["api"], idle: 2)
    #expect(quiet.isQuiet && !busy.isQuiet)
    let w = LiveShowMode.working
    #expect(w.decide(quiet, cardShowing: false, quietSince: nil, now: now) == .none, "an all-idle board starts nothing")
    #expect(w.decide(busy, cardShowing: false, quietSince: nil, now: now) == .show)
    #expect(w.decide(quiet, cardShowing: true, quietSince: now.addingTimeInterval(-60), now: now) == .show)
    #expect(w.decide(quiet, cardShowing: true, quietSince: now.addingTimeInterval(-15 * 60), now: now) == .end)
    #expect(LiveShowMode.always.decide(quiet, cardShowing: false, quietSince: now.addingTimeInterval(-9999), now: now) == .show)
}

@Test func ofSeveralCardsTheNewestIsKept() {
    let now = Date(timeIntervalSince1970: 10_000)
    let old = now.addingTimeInterval(-7200)
    #expect(LiveCards.keep([], started: [:], now: now) == nil)
    #expect(LiveCards.keep(["a", "b"], started: ["a": old, "b": now.addingTimeInterval(-60)], now: now) == "b")
    #expect(LiveCards.keep(["b", "a"], started: ["a": old, "b": now.addingTimeInterval(-60)], now: now) == "b")
    #expect(LiveCards.keep(["a", "hub"], started: ["a": old], now: now) == "hub", "one the hub started is new")
    #expect(LiveCards.keep(["x", "y"], started: [:], now: now) == "y", "a tie keeps the later")
}
