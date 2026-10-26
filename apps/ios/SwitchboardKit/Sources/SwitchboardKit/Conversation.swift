import Foundation

/// An agent's conversation as the app shows it, built up from transcript pages: the latest page
/// first, new items as they're written, older pages on request.
public struct Conversation: Equatable, Sendable {
    public typealias Item = Schemas.TranscriptItem

    public private(set) var items: [Item] = []
    /// Where the earliest loaded page starts in the log; nil until the first page arrives.
    public private(set) var start: Int64?
    /// Where to continue from.
    public private(set) var end: Int64 = 0
    /// Tool results whose call is on a page not loaded yet.
    private var results: [String: Item] = [:]

    public init() {}

    public var isLoaded: Bool { start != nil }
    public var hasEarlier: Bool { (start ?? 0) > 0 }

    /// The latest page, in place of everything loaded so far.
    public mutating func replace(with page: Schemas.Transcript) {
        items = []
        results = [:]
        start = page.start
        end = page.end
        merge(page.items, atFront: false)
    }

    /// A page asked for after `end`. One that doesn't start there skipped some, so it replaces.
    public mutating func append(_ page: Schemas.Transcript, askedAfter after: Int64) {
        guard page.start == after else { return replace(with: page) }
        end = page.end
        merge(page.items, atFront: false)
    }

    /// A page asked for before `start`.
    public mutating func prepend(_ page: Schemas.Transcript) {
        start = page.start
        merge(page.items, atFront: true)
    }

    private mutating func merge(_ page: [Item], atFront: Bool) {
        var added: [Item] = []
        for item in page {
            if let i = items.firstIndex(where: { $0.id == item.id }) {
                items[i] = Self.combine(items[i], item)
            } else if let i = added.firstIndex(where: { $0.id == item.id }) {
                added[i] = Self.combine(added[i], item)
            } else if item.text == nil {
                results[item.id] = item
            } else {
                added.append(results.removeValue(forKey: item.id).map { Self.combine(item, $0) } ?? item)
            }
        }
        items = atFront ? added + items : items + added
    }

    /// A later copy replaces an item; a bare result fills in its call.
    private static func combine(_ old: Item, _ new: Item) -> Item {
        guard new.text == nil else { return new }
        var item = old
        item.output = new.output
        item.done = new.done
        item.failed = new.failed
        return item
    }
}
