import Foundation
import Testing

@testable import SwitchboardKit

@Test func longUnchangedRunsFoldButKeepThreeLinesBesideEachChange() {
    let kinds = Array(repeating: " ", count: 10) + ["+"] + Array(repeating: " ", count: 20) + ["-"] + Array(repeating: " ", count: 4)
    let rows = DiffFolding.rows(kinds)
    #expect(rows.first == .fold(range: 0..<7), "a hunk's leading run keeps only the three nearest its change")
    #expect(rows.contains(.fold(range: 14..<28)))
    #expect(!rows.contains { if case .fold(let r) = $0 { r.lowerBound > 30 } else { false } }, "a short trailing run stays")
    let shown = DiffFolding.rows(kinds, open: [14])
    #expect(!shown.contains(.fold(range: 14..<28)))
    #expect(shown.filter { if case .line = $0 { true } else { false } }.count == rows.filter { if case .line = $0 { true } else { false } }.count + 14)
    #expect(DiffFolding.rows([" ", "+", " "]) == [.line(index: 0), .line(index: 1), .line(index: 2)])
}

@Test func askingQuotesTheFileTheLinesAndTheLinesThemselves() {
    let m = DiffQuote.message(path: "src/importer/retry.ts", lines: [(42, "const delay = 2 ** attempt * 250;"), (43, "await sleep(delay);")],
                              question: " Cap the delay at 30 seconds. ")
    #expect(m == "src/importer/retry.ts:42–43:\n```\nconst delay = 2 ** attempt * 250;\nawait sleep(delay);\n```\n\nCap the delay at 30 seconds.")
    #expect(DiffQuote.location(path: "a.rs", numbers: [7]) == "a.rs:7")
}
