import Foundation

/// A diff hunk as the phone draws it: lines, with long runs of unchanged ones folded.
public enum DiffRow: Equatable, Sendable, Identifiable {
    case line(index: Int)
    /// Lines `range` of the hunk, unchanged and hidden until tapped.
    case fold(range: Range<Int>)

    public var id: String {
        switch self {
        case .line(let i): "l\(i)"
        case .fold(let r): "f\(r.lowerBound)"
        }
    }
}

public enum DiffFolding {
    /// Unchanged lines kept beside each change.
    public static let keep = 3
    /// A run is folded only when at least this many lines would hide.
    public static let minFold = 6

    /// `kinds` are the hunk's line kinds (`" "`, `"+"`, `"-"`); `open` lists folds already shown.
    public static func rows(_ kinds: [String], open: Set<Int> = []) -> [DiffRow] {
        var out: [DiffRow] = []
        var i = 0
        while i < kinds.count {
            guard kinds[i] == " " else {
                out.append(.line(index: i))
                i += 1
                continue
            }
            var j = i
            while j < kinds.count, kinds[j] == " " { j += 1 }
            let lead = i == 0 ? 0 : keep
            let trail = j == kinds.count ? 0 : keep
            let hideFrom = i + lead, hideTo = j - trail
            if hideTo - hideFrom >= minFold, !open.contains(hideFrom) {
                out += (i..<hideFrom).map { .line(index: $0) }
                out.append(.fold(range: hideFrom..<hideTo))
                out += (hideTo..<j).map { .line(index: $0) }
            } else {
                out += (i..<j).map { .line(index: $0) }
            }
            i = j
        }
        return out
    }
}

/// The message "Ask agent" sends: the file, the line numbers and the lines, then the question.
public enum DiffQuote {
    /// `lines` are (number, text) in the new file, or the old one for deleted lines.
    public static func message(path: String, lines: [(Int, String)], question: String) -> String {
        "\(location(path: path, numbers: lines.map(\.0))):\n```\n\(lines.map(\.1).joined(separator: "\n"))\n```\n\n\(question.trimmingCharacters(in: .whitespacesAndNewlines))"
    }

    /// `src/a.ts:42–43`, or `src/a.ts:42` for one line.
    public static func location(path: String, numbers: [Int]) -> String {
        guard let lo = numbers.min(), let hi = numbers.max() else { return path }
        return lo == hi ? "\(path):\(lo)" : "\(path):\(lo)–\(hi)"
    }
}

/// Bytes as the Changes list shows a binary file's size.
public func fileSize(_ bytes: Int64) -> String {
    let f = ByteCountFormatter()
    f.countStyle = .file
    return f.string(fromByteCount: bytes)
}
