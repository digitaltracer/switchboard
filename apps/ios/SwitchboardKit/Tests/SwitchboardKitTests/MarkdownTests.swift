import Testing
@testable import SwitchboardKit

@Test func listsKeepTheirNestingAndWrappedLinesJoinTheirItem() {
    let text = """
    ## Round 2

    ### Findings

    I found no high defects.
    - **r2-1** · `client.py:181` · low
      - State: a retried call
      with an expired token.
      - Fix (a): say so.
    - **r2-2** · test gap

    1. First
    2. Second
    ---
    After.
    """
    #expect(Markdown.blocks(text) == [
        .heading(level: 2, text: "Round 2"),
        .heading(level: 3, text: "Findings"),
        .paragraph("I found no high defects."),
        .list([
            .init(level: 0, marker: "•", text: "**r2-1** · `client.py:181` · low"),
            .init(level: 1, marker: "•", text: "State: a retried call with an expired token."),
            .init(level: 1, marker: "•", text: "Fix (a): say so."),
            .init(level: 0, marker: "•", text: "**r2-2** · test gap"),
            .init(level: 0, marker: "1.", text: "First"),
            .init(level: 0, marker: "2.", text: "Second"),
        ]),
        .rule,
        .paragraph("After."),
    ])
}

@Test func codeAndTablesStayWhole() {
    let text = "Run:\n```\n- not a list\n```\n| a | b |\n|---|---|\n| 1 | 2 |\nAfter"
    #expect(Markdown.blocks(text) == [
        .paragraph("Run:"),
        .code("- not a list"),
        .table("| a | b |\n|---|---|\n| 1 | 2 |"),
        .paragraph("After"),
    ])
}

@Test func aParagraphAfterABlankLineEndsTheList() {
    #expect(Markdown.blocks("- one\n\nThen this.") == [.list([.init(level: 0, marker: "•", text: "one")]), .paragraph("Then this.")])
}
