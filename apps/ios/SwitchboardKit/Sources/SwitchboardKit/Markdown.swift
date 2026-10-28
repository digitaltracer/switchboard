import Foundation

/// An agent's Markdown in the blocks a phone draws: paragraphs, headings, lists with their nesting,
/// code, tables and rules. Inline styles stay in each block's text.
public enum Markdown {
    public enum Block: Equatable, Sendable {
        case paragraph(String)
        /// `#` is 1, `##` 2 and so on.
        case heading(level: Int, text: String)
        case list([Item])
        case code(String)
        case table(String)
        case rule
    }

    /// `marker` is `•` for `-`, `*` and `+`, else the number as written, like `2.`.
    public struct Item: Equatable, Sendable {
        public let level: Int
        public let marker: String
        public let text: String

        public init(level: Int, marker: String, text: String) {
            self.level = level
            self.marker = marker
            self.text = text
        }
    }

    public static func blocks(_ text: String) -> [Block] {
        var out: [Block] = []
        var para: [String] = []
        var table: [String] = []
        var items: [Item] = []
        // The indents seen in this list, shallowest first; an item's level is where its indent ranks.
        var indents: [Int] = []
        var code: [String]?
        var blank = false
        func flush() {
            if !para.isEmpty { out.append(.paragraph(para.joined(separator: "\n"))); para = [] }
            if !table.isEmpty { out.append(.table(table.joined(separator: "\n"))); table = [] }
            if !items.isEmpty { out.append(.list(items)); items = [] }
            indents = []
        }
        for line in text.components(separatedBy: "\n") {
            let t = line.trimmingCharacters(in: .whitespaces)
            let afterBlank = blank
            blank = code == nil && t.isEmpty
            if t.hasPrefix("```") {
                if let c = code { out.append(.code(c.joined(separator: "\n"))); code = nil } else { flush(); code = [] }
            } else if code != nil {
                code?.append(line)
            } else if t.hasPrefix("|") {
                if !para.isEmpty || !items.isEmpty { flush() }
                table.append(t)
            } else if isRule(t) {
                flush()
                out.append(.rule)
            } else if t.hasPrefix("#") {
                flush()
                let level = min(t.prefix { $0 == "#" }.count, 6)
                out.append(.heading(level: level, text: t.drop { $0 == "#" }.trimmingCharacters(in: .whitespaces)))
            } else if t.isEmpty {
                if items.isEmpty { flush() }
            } else if let (indent, marker, rest) = item(line) {
                if !para.isEmpty || !table.isEmpty { flush() }
                while let last = indents.last, last > indent { indents.removeLast() }
                if indents.last.map({ $0 < indent }) ?? true { indents.append(indent) }
                items.append(Item(level: indents.count - 1, marker: marker, text: rest))
            } else if !items.isEmpty, !afterBlank || line.hasPrefix(" ") {
                // A line under an item continues it; after a blank line, only an indented one does.
                let last = items.removeLast()
                items.append(Item(level: last.level, marker: last.marker, text: last.text + " " + t))
            } else {
                if !items.isEmpty || !table.isEmpty { flush() }
                para.append(line)
            }
        }
        if let c = code { out.append(.code(c.joined(separator: "\n"))) }
        flush()
        return out
    }

    /// `- x`, `* x`, `+ x`, `1. x` or `1) x`: the indent, the marker and the rest.
    private static func item(_ line: String) -> (Int, String, String)? {
        let indent = line.prefix { $0 == " " }.count
        let rest = line.dropFirst(indent)
        if let first = rest.first, "-*+".contains(first), rest.dropFirst().first == " " {
            return (indent, "•", rest.dropFirst(2).trimmingCharacters(in: .whitespaces))
        }
        let digits = rest.prefix { $0.isNumber }
        guard (1...3).contains(digits.count) else { return nil }
        let after = rest.dropFirst(digits.count)
        guard let mark = after.first, mark == "." || mark == ")", after.dropFirst().first == " " else { return nil }
        return (indent, String(digits) + String(mark), after.dropFirst(2).trimmingCharacters(in: .whitespaces))
    }

    /// `---`, `***` or `___`, spaces allowed between.
    private static func isRule(_ t: String) -> Bool {
        let chars = t.filter { $0 != " " }
        guard let first = chars.first, "-*_".contains(first), chars.count >= 3 else { return false }
        return chars.allSatisfy { $0 == first }
    }
}
