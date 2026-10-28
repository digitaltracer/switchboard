import Testing
@testable import SwitchboardKit

@Test func aPromptKeepsItsShadedBand() {
    let runs = Ansi.parse("\u{1b}[0m\u{1b}[38;2;153;153;153m\u{1b}[48;2;55;55;55m❯ \u{1b}[0m\u{1b}[38;2;255;255;255m\u{1b}[48;2;55;55;55mpush both\u{1b}[0m\n  done")
    #expect(runs.map(\.text) == ["❯ ", "push both", "\n  done"])
    #expect(runs[1].style.bg == Ansi.Color(r: 55, g: 55, b: 55))
    #expect(runs[1].style.fg == Ansi.Color(r: 255, g: 255, b: 255))
    #expect(runs[2].style == Ansi.Style())
}

@Test func paletteBoldAndOtherCodes() {
    let runs = Ansi.parse("\u{1b}[1;38;5;196mred\u{1b}[22;39m plain \u{1b}[92mgreen\u{1b}[2Jgone")
    #expect(runs.map(\.text) == ["red", " plain ", "greengone"])
    #expect(runs[0].style.bold && runs[0].style.fg == Ansi.Color(r: 255, g: 0, b: 0))
    #expect(!runs[1].style.bold && runs[1].style.fg == nil)
    #expect(runs[2].style.fg == Ansi.palette(10))
}
