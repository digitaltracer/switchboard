import Testing
@testable import SwitchboardKit

@Test func aTableKeepsItsHeaderAndSizesColumnsByTheirLongestCell() throws {
    let t = try #require(MarkdownTable.parse("""
    | Fix | Commit |
    |---|:---:|
    | The FastAPI recipe now warns that it stops the middleware's exceptions | `41062ee` |
    | Pipes \\| stay | `a1b2c3d` |
    | Short |
    """))
    #expect(t.header == ["Fix", "Commit"])
    #expect(t.rows.count == 3)
    #expect(t.rows[1][0] == "Pipes | stay")
    #expect(t.rows[2] == ["Short", ""], "a short row is padded")
    #expect(t.weights == [40, 9], "the long column is capped, the hash column stays narrow")
}

@Test func aTableWithoutARuleHasNoHeader() throws {
    let t = try #require(MarkdownTable.parse("| a | b |\n| c | d |"))
    #expect(t.header.isEmpty && t.rows.count == 2)
    #expect(MarkdownTable.parse("no table here") == nil)
}
