import Foundation

/// A terminal screen's colours, from ANSI SGR codes. The hub sends only colour codes; anything
/// else is dropped.
public enum Ansi {
    public struct Color: Equatable, Sendable {
        public let r, g, b: UInt8
    }

    public struct Style: Equatable, Sendable {
        public var fg: Color?
        public var bg: Color?
        public var bold = false
        public var dim = false
        public var italic = false
        public var underline = false
        public var inverse = false
    }

    public struct Run: Equatable, Sendable {
        public let text: String
        public let style: Style
    }

    /// The text split into runs of one style each.
    public static func parse(_ s: String) -> [Run] {
        var runs: [Run] = []
        var style = Style()
        var text = ""
        func flush() {
            if !text.isEmpty { runs.append(Run(text: text, style: style)) }
            text = ""
        }
        var chars = s.unicodeScalars.makeIterator()
        while let c = chars.next() {
            guard c == "\u{1b}" else {
                text.unicodeScalars.append(c)
                continue
            }
            guard chars.next() == "[" else { continue }
            var params = ""
            var final: Unicode.Scalar?
            while let p = chars.next() {
                if (0x40...0x7e).contains(p.value) { final = p; break }
                params.unicodeScalars.append(p)
            }
            guard final == "m" else { continue }
            flush()
            apply(params, to: &style)
        }
        flush()
        return runs
    }

    static func apply(_ params: String, to style: inout Style) {
        var codes = params.split(separator: ";", omittingEmptySubsequences: false).map { Int($0) ?? 0 }[...]
        if codes.isEmpty { codes = [0] }
        while let code = codes.popFirst() {
            switch code {
            case 0: style = Style()
            case 1: style.bold = true
            case 2: style.dim = true
            case 3: style.italic = true
            case 4: style.underline = true
            case 7: style.inverse = true
            case 22: style.bold = false; style.dim = false
            case 23: style.italic = false
            case 24: style.underline = false
            case 27: style.inverse = false
            case 30...37: style.fg = palette(code - 30)
            case 90...97: style.fg = palette(code - 90 + 8)
            case 39: style.fg = nil
            case 40...47: style.bg = palette(code - 40)
            case 100...107: style.bg = palette(code - 100 + 8)
            case 49: style.bg = nil
            case 38, 48:
                let color = extended(&codes)
                if code == 38 { style.fg = color } else { style.bg = color }
            default: break
            }
        }
    }

    /// `5;n` from the 256-colour palette or `2;r;g;b`.
    private static func extended(_ codes: inout ArraySlice<Int>) -> Color? {
        switch codes.popFirst() {
        case 5: return codes.popFirst().map(palette)
        case 2:
            guard codes.count >= 3 else { return nil }
            let c = codes.prefix(3).map { UInt8(clamping: $0) }
            codes = codes.dropFirst(3)
            return Color(r: c[0], g: c[1], b: c[2])
        default: return nil
        }
    }

    /// xterm's 256 colours.
    static func palette(_ n: Int) -> Color {
        let base: [(UInt8, UInt8, UInt8)] = [
            (0, 0, 0), (205, 49, 49), (13, 188, 121), (229, 229, 16), (36, 114, 200), (188, 63, 188), (17, 168, 205), (229, 229, 229),
            (102, 102, 102), (241, 76, 76), (35, 209, 139), (245, 245, 67), (59, 142, 234), (214, 112, 214), (41, 184, 219), (255, 255, 255),
        ]
        switch n {
        case 0..<16: return Color(r: base[n].0, g: base[n].1, b: base[n].2)
        case 16..<232:
            let i = n - 16
            let level = { (v: Int) -> UInt8 in v == 0 ? 0 : UInt8(55 + v * 40) }
            return Color(r: level(i / 36), g: level(i / 6 % 6), b: level(i % 6))
        case 232..<256:
            let v = UInt8(8 + (n - 232) * 10)
            return Color(r: v, g: v, b: v)
        default: return Color(r: 229, g: 229, b: 229)
        }
    }
}
