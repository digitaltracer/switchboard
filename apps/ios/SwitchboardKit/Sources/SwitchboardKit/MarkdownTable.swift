import Foundation

/// A Markdown table's cells, with each column's share of the width from its longest cell.
public struct MarkdownTable: Equatable, Sendable {
    public let header: [String]
    public let rows: [[String]]
    public let weights: [Double]

    public var columns: Int { weights.count }

    /// `| a | b |` lines, the `|---|` line under the header marking it. Nil when no line has cells.
    public static func parse(_ text: String) -> MarkdownTable? {
        var lines = text.split(separator: "\n").map { cells(String($0)) }.filter { !$0.isEmpty }
        guard !lines.isEmpty else { return nil }
        var header: [String] = []
        if lines.count > 1, lines[1].allSatisfy(isRule) {
            header = lines[0]
            lines.removeFirst(2)
        }
        let columns = ([header] + lines).map(\.count).max() ?? 0
        let pad = { (r: [String]) in r + Array(repeating: "", count: columns - r.count) }
        let rows = lines.map(pad)
        let all = (header.isEmpty ? [] : [pad(header)]) + rows
        let weights = (0..<columns).map { c in Double(min(max(all.map { $0[c].count }.max() ?? 0, 3), 40)) }
        return MarkdownTable(header: header.isEmpty ? [] : pad(header), rows: rows, weights: weights)
    }

    /// A row's cells; `\|` is a literal bar.
    static func cells(_ line: String) -> [String] {
        var s = line.trimmingCharacters(in: .whitespaces)
        guard s.hasPrefix("|") else { return [] }
        s.removeFirst()
        if s.hasSuffix("|") && !s.hasSuffix("\\|") { s.removeLast() }
        let marker = "\u{0}"
        return s.replacingOccurrences(of: "\\|", with: marker)
            .split(separator: "|", omittingEmptySubsequences: false)
            .map { $0.replacingOccurrences(of: marker, with: "|").trimmingCharacters(in: .whitespaces) }
    }

    static func isRule(_ cell: String) -> Bool {
        !cell.isEmpty && cell.allSatisfy { $0 == "-" || $0 == ":" } && cell.contains("-")
    }
}
