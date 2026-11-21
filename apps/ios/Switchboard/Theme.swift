import SwiftUI
import SwitchboardKit
import UIKit

/// The Inbox design's colours, light and dark: a near-white ground, black primary buttons, one
/// blue for working and links, orange for needs you, green for done.
enum Ink {
    static let bg = adaptive(0xF5F6F6, 0x0C0D0F)
    static let sf = adaptive(0xFFFFFF, 0x16181B)
    static let ink = adaptive(0x101214, 0xF2F3F4)
    static let sub = adaptive(0x4F555B, 0xA9AEB4)
    static let mut = adaptive(0x666C73, 0x8C9299)
    static let line = adaptive(0xE1E3E5, 0x25282C)
    static let fill = adaptive(0xECEEEF, 0x1E2125)
    static let ac = adaptive(0x2743D6, 0x93A5FF)
    static let warn = adaptive(0xB4460A, 0xFFA468)
    static let wbg = adaptive(0xFFF1E6, 0x2B1C11)
    static let ok = adaptive(0x1F7A4D, 0x6FD3A0)
    static let bad = adaptive(0xC0362C, 0xFF8F85)
    static let inv = adaptive(0xFFFFFF, 0x0C0D0F)
    /// Behind code and the terminal, in both appearances.
    static let code = adaptive(0x101214, 0x000000)
    static let codeText = Color(hex: 0xF2F3F4)
    static let codeDim = Color(hex: 0x8C9299)
    static let term = Color(hex: 0x0C0D0F)

    private static func adaptive(_ light: UInt32, _ dark: UInt32) -> Color {
        Color(UIColor { $0.userInterfaceStyle == .dark ? UIColor(hex: dark) : UIColor(hex: light) })
    }
}

extension UIColor {
    convenience init(hex: UInt32) {
        self.init(red: CGFloat((hex >> 16) & 0xFF) / 255, green: CGFloat((hex >> 8) & 0xFF) / 255,
                  blue: CGFloat(hex & 0xFF) / 255, alpha: 1)
    }
}

extension Color {
    init(hex: UInt32) { self.init(UIColor(hex: hex)) }
}

/// Geist for text and Geist Mono for commands, paths and fingerprints, both bundled. Sizes follow
/// Dynamic Type from the design's size at the default setting.
enum Face {
    static func geist(_ size: CGFloat, _ weight: Font.Weight = .regular, relativeTo style: Font.TextStyle = .body) -> Font {
        .custom(name("Geist", weight), size: size, relativeTo: style)
    }

    /// At exactly `size`, for text whose size is already scaled, such as a reply's.
    static func fixed(_ size: CGFloat, _ weight: Font.Weight = .regular, mono: Bool = false) -> Font {
        .custom(name(mono ? "GeistMono" : "Geist", mono && weight == .bold ? .semibold : weight), fixedSize: size)
    }

    static func mono(_ size: CGFloat, _ weight: Font.Weight = .regular, relativeTo style: Font.TextStyle = .footnote) -> Font {
        .custom(name("GeistMono", weight == .bold ? .semibold : weight), size: size, relativeTo: style)
    }

    private static func name(_ family: String, _ w: Font.Weight) -> String {
        switch w {
        case .medium: "\(family)-Medium"
        case .semibold: "\(family)-SemiBold"
        case .bold, .heavy, .black: "\(family)-Bold"
        default: "\(family)-Regular"
        }
    }

    /// The sentence at the top of a page, "3 things need you."
    static let state = geist(34, .semibold, relativeTo: .largeTitle)
    static let h1 = geist(28, .semibold, relativeTo: .title)
    static let title = geist(16, .medium, relativeTo: .body)
    static let body = geist(15, relativeTo: .body)
    static let sub = geist(13.5, relativeTo: .subheadline)
    static let meta = geist(13, relativeTo: .footnote)
    static let section = geist(12, .medium, relativeTo: .caption)
    static let button = geist(16, .semibold, relativeTo: .headline)
    static let say = geist(16, relativeTo: .body)
    static let draft = geist(22, .medium, relativeTo: .title2)
}

extension View {
    /// The headline sentence: tight, with its second part quieter.
    func stateStyle(size: CGFloat = 34) -> some View {
        font(Face.geist(size, .semibold, relativeTo: .largeTitle)).tracking(-size * 0.035).foregroundStyle(Ink.ink)
    }
}

/// A headline in two sentences, "3 things need you. 2 agents working.", the second quieter. When
/// both don't fit on one line, the second starts its own.
struct Headline: View {
    let main: String
    var quiet: String?

    init(_ main: String, _ quiet: String? = nil) {
        self.main = main
        self.quiet = quiet
    }

    var body: some View {
        if let quiet, !quiet.isEmpty {
            ViewThatFits(in: .horizontal) {
                (Text(main + " ") + Text(quiet).foregroundColor(Ink.mut)).lineLimit(1)
                VStack(alignment: .leading, spacing: 0) {
                    Text(main).fixedSize(horizontal: false, vertical: true)
                    Text(quiet).foregroundColor(Ink.mut).fixedSize(horizontal: false, vertical: true)
                }
            }
            .accessibilityElement(children: .combine)
        } else {
            Text(main)
        }
    }
}

// Status, never by colour alone: each has its own glyph.

enum Glyph: Equatable {
    case need, work, done, idle, end

    init(agentStatus: String) {
        switch agentStatus {
        case "blocked": self = .need
        case "working": self = .work
        case "done": self = .done
        case "ended", "stopped", "exited", "failed", "deleted": self = .end
        default: self = .idle
        }
    }
}

struct StatusGlyph: View {
    let glyph: Glyph
    var size: CGFloat = 18

    var body: some View {
        let w = size * 1.7 / 18
        ZStack {
            switch glyph {
            case .need:
                RoundedRectangle(cornerRadius: size * 4 / 18)
                    .fill(Ink.warn)
                    .frame(width: size * 0.76, height: size * 0.76)
                    .rotationEffect(.degrees(45))
            case .work:
                Circle().trim(from: 0, to: 0.75)
                    .stroke(Ink.ac, style: StrokeStyle(lineWidth: w, lineCap: .round))
                    .rotationEffect(.degrees(-45))
                    .padding(w / 2)
            case .done:
                Circle().stroke(Ink.ok, lineWidth: w).padding(w / 2)
                Path { p in
                    p.move(to: CGPoint(x: size * 0.30, y: size * 0.52))
                    p.addLine(to: CGPoint(x: size * 0.45, y: size * 0.66))
                    p.addLine(to: CGPoint(x: size * 0.70, y: size * 0.36))
                }
                .stroke(Ink.ok, style: StrokeStyle(lineWidth: w, lineCap: .round, lineJoin: .round))
            case .idle:
                Circle().stroke(Ink.mut, lineWidth: w).padding(w / 2)
            case .end:
                Circle().stroke(Ink.mut, style: StrokeStyle(lineWidth: w, dash: [size * 0.16, size * 0.12])).padding(w / 2)
            }
        }
        .frame(width: size, height: size)
        .accessibilityHidden(true)
    }
}

/// Where an agent lives: the Herdr app or a Herdr terminal, by shape as well as word.
struct WhereTag: View {
    let backend: String

    var body: some View {
        let app = backend == "hc"
        HStack(spacing: 5) {
            if app {
                RoundedRectangle(cornerRadius: 2)
                    .strokeBorder(Ink.sub, lineWidth: 1.4)
                    .overlay(alignment: .top) { Rectangle().fill(Ink.sub).frame(height: 3) }
                    .clipShape(RoundedRectangle(cornerRadius: 2))
                    .frame(width: 10, height: 8)
            } else {
                Text(">_").font(Face.mono(10, .medium)).tracking(-0.5)
            }
            Text(app ? "App" : "Terminal")
        }
        .font(Face.geist(12, .medium, relativeTo: .caption))
        .foregroundStyle(Ink.sub)
        .padding(.horizontal, 8)
        .frame(height: 22)
        .background(Ink.sf, in: RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Ink.line))
        .fixedSize()
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(app ? "In the Herdr app" : "In a Herdr terminal")
    }
}

/// "Working", "Needs you" as a small rounded chip with its glyph.
struct StatusChip: View {
    let status: SessionStatus

    var body: some View {
        HStack(spacing: 6) {
            StatusGlyph(glyph: status.glyph, size: 13)
            Text(status.text)
        }
        .font(Face.geist(13, .medium, relativeTo: .footnote))
        .foregroundStyle(status.glyph == .need ? Ink.warn : status.glyph == .work ? Ink.ac : Ink.sub)
        .padding(.horizontal, 10)
        .frame(height: 28)
        .background(status.glyph == .need ? Ink.wbg : Ink.fill, in: Capsule())
        .fixedSize()
    }
}

struct Chip: View {
    let text: String
    var on = false

    var body: some View {
        Text(text)
            .font(Face.geist(13, .medium, relativeTo: .footnote))
            .lineLimit(1)
            .foregroundStyle(on ? Ink.inv : Ink.sub)
            .padding(.horizontal, 10)
            .frame(height: 28)
            .background(on ? Ink.ink : Ink.fill, in: Capsule())
            .fixedSize()
    }
}

/// The uppercase label over a list, with an optional count or link at its end.
struct SectionHead<Trailing: View>: View {
    let title: String
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title.uppercased())
            Spacer(minLength: 8)
            trailing
        }
        .font(Face.section)
        .tracking(0.84)
        .foregroundStyle(Ink.mut)
        .padding(.top, 22)
        .padding(.bottom, 8)
        .accessibilityAddTraits(.isHeader)
    }
}

extension SectionHead where Trailing == Text {
    init(_ title: String, count: Int? = nil) {
        self.title = title
        trailing = Text(count.map(String.init) ?? "")
    }
}

/// A flat row with a hairline above it, as every list draws them.
struct Row<Leading: View, Trailing: View>: View {
    let title: String
    var subtitle: String?
    var subtitleMono = false
    var subtitleColor: Color = Ink.sub
    var dim = false
    var first = false
    @ViewBuilder var leading: Leading
    @ViewBuilder var trailing: Trailing
    @Environment(\.dynamicTypeSize) private var typeSize

    var body: some View {
        HStack(spacing: 12) {
            leading
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                    .font(dim ? Face.geist(16, relativeTo: .body) : Face.title)
                    .tracking(-0.16)
                    .foregroundStyle(dim ? Ink.mut : Ink.ink)
                    .lineLimit(typeSize.isAccessibilitySize ? 3 : 1)
                if let subtitle, !subtitle.isEmpty {
                    Text(subtitle)
                        .font(subtitleMono ? Face.mono(12.5) : Face.sub)
                        .foregroundStyle(dim ? Ink.mut : subtitleColor)
                        .lineLimit(typeSize.isAccessibilitySize ? 2 : 1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            trailing
        }
        .padding(.vertical, 12)
        .frame(minHeight: 60)
        .overlay(alignment: .top) { if !first { Hairline() } }
        .contentShape(Rectangle())
    }
}

extension Row where Leading == EmptyView, Trailing == EmptyView {
    init(title: String, subtitle: String? = nil, first: Bool = false) {
        self.init(title: title, subtitle: subtitle, first: first, leading: { EmptyView() }, trailing: { EmptyView() })
    }
}

struct Hairline: View {
    var body: some View { Rectangle().fill(Ink.line).frame(height: 1) }
}

struct Chevron: View {
    var down = false

    var body: some View {
        Image(systemName: down ? "chevron.down" : "chevron.right")
            .font(.system(size: 12, weight: .semibold))
            .foregroundStyle(Ink.mut)
            .accessibilityHidden(true)
    }
}

/// A label and its value on one line, as Details and At your desk show them.
struct FieldRow<Value: View>: View {
    let label: String
    var first = false
    @ViewBuilder var value: Value

    var body: some View {
        HStack(spacing: 12) {
            Text(label).font(Face.geist(16, relativeTo: .body)).foregroundStyle(Ink.ink)
            Spacer(minLength: 8)
            value
                .font(Face.geist(16, relativeTo: .body))
                .foregroundStyle(Ink.sub)
                .lineLimit(1)
                .multilineTextAlignment(.trailing)
        }
        .padding(.vertical, 11)
        .frame(minHeight: 50)
        .overlay(alignment: .top) { if !first { Hairline() } }
    }
}

extension FieldRow where Value == Text {
    init(_ label: String, value: String, mono: Bool = false, first: Bool = false) {
        self.label = label
        self.first = first
        self.value = Text(value).font(mono ? Face.mono(12.5) : Face.geist(16, relativeTo: .body))
    }
}

/// A 52-point button: black primary, outlined, or greyed when it can't be used yet.
struct BigButtonStyle: ButtonStyle {
    enum Kind { case primary, outline, plain }
    var kind: Kind = .primary
    var destructive = false

    func makeBody(configuration: Configuration) -> some View {
        Inner(configuration: configuration, kind: kind, destructive: destructive)
    }

    private struct Inner: View {
        @Environment(\.isEnabled) private var enabled
        let configuration: Configuration
        let kind: Kind
        let destructive: Bool

        var body: some View {
            let fg: Color = !enabled && kind == .primary ? Ink.mut : destructive ? Ink.bad : kind == .primary ? Ink.inv : Ink.ink
            let bg: Color = kind == .primary ? (enabled ? Ink.ink : Ink.fill) : kind == .outline ? Ink.sf : .clear
            configuration.label
                .font(Face.button)
                .lineLimit(1)
                .foregroundStyle(fg)
                .padding(.horizontal, 16)
                .frame(maxWidth: .infinity, minHeight: 52)
                .background(bg, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                .overlay {
                    if kind == .outline { RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Ink.line, lineWidth: 1.5) }
                }
                .opacity(configuration.isPressed ? 0.7 : (!enabled && kind != .primary ? 0.5 : 1))
                .contentShape(Rectangle())
        }
    }
}

/// The round black mic.
struct MicDisc: View {
    var size: CGFloat = 40

    var body: some View {
        Image(systemName: "mic")
            .font(.system(size: size * 0.45, weight: .medium))
            .frame(width: size, height: size)
            .background(Ink.ink, in: Circle())
            .foregroundStyle(Ink.inv)
    }
}

/// A 52-point outlined circle, for Cancel, Keep talking and Type instead.
struct RoundIcon: View {
    let symbol: String

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: 19, weight: .medium))
            .frame(width: 52, height: 52)
            .overlay(Circle().strokeBorder(Ink.line, lineWidth: 1.5))
            .foregroundStyle(Ink.ink)
            .contentShape(Circle())
    }
}

/// A card: the approval, the fingerprint.
struct CardBox<Content: View>: View {
    var padding: CGFloat = 16
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 12) { content }
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Ink.sf, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(Ink.line))
    }
}

/// A command or input, white on near-black in both appearances.
struct CodeBlock: View {
    let text: String
    var wraps = false

    var body: some View {
        Text(text)
            .font(Face.mono(13))
            .foregroundStyle(Ink.codeText)
            .lineLimit(wraps ? nil : 1)
            .truncationMode(.tail)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14)
            .padding(.vertical, 11)
            .background(Ink.code, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay { RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Ink.line.opacity(0.6)) }
            .textSelection(.enabled)
    }
}

/// A warm notice: showing what was known, a notification that opened nothing.
struct Banner: View {
    let text: String

    var body: some View {
        Text(text)
            .font(Face.geist(13.5, .medium, relativeTo: .footnote))
            .foregroundStyle(Ink.warn)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .background(Ink.wbg, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

/// A radio row: a listed option, an "Open it in" choice, a destination. One not on offer is
/// dashed and greyed.
struct RadioRow<Detail: View>: View {
    let title: String
    let on: Bool
    var offered = true
    var first = false
    @ViewBuilder var detail: Detail

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            ZStack {
                if on {
                    Circle().fill(Ink.ink)
                    Image(systemName: "checkmark").font(.system(size: 10, weight: .bold)).foregroundStyle(Ink.inv)
                } else {
                    Circle().strokeBorder(Ink.mut, style: StrokeStyle(lineWidth: 1.7, dash: offered ? [] : [3, 3]))
                }
            }
            .frame(width: 22, height: 22)
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(offered ? Face.title : Face.geist(16, relativeTo: .body))
                    .foregroundStyle(offered ? Ink.ink : Ink.mut)
                    .multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
                detail
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.vertical, 15)
        .frame(minHeight: 56)
        .overlay(alignment: .top) { if !first { Hairline() } }
        .contentShape(Rectangle())
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}

extension RadioRow where Detail == EmptyView {
    init(title: String, on: Bool, offered: Bool = true, first: Bool = false) {
        self.init(title: title, on: on, offered: offered, first: first) { EmptyView() }
    }
}

/// A quiet line under a radio row.
struct Note: View {
    let text: String
    var color: Color = Ink.mut

    var body: some View {
        Text(text).font(Face.meta).lineSpacing(2).foregroundStyle(color).fixedSize(horizontal: false, vertical: true)
    }
}

/// The three text tabs: Now, Spaces, Projects; Summary, Conversation, Screen.
struct TextTabs<T: Hashable>: View {
    let tabs: [(T, String)]
    @Binding var selection: T

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
        HStack(spacing: 22) {
            ForEach(tabs, id: \.0) { tab, label in
                Button { selection = tab } label: {
                    Text(label)
                        .font(Face.geist(15, .medium, relativeTo: .body))
                        .lineLimit(1)
                        .fixedSize()
                        .foregroundStyle(selection == tab ? Ink.ink : Ink.mut)
                        .frame(minHeight: 44)
                        .overlay(alignment: .bottom) {
                            if selection == tab { Rectangle().fill(Ink.ink).frame(height: 2) }
                        }
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityAddTraits(selection == tab ? .isSelected : [])
            }
        }
        .padding(.horizontal, 20)
        }
        .scrollBounceBehavior(.basedOnSize, axes: .horizontal)
        .padding(.top, 8)
        .overlay(alignment: .bottom) { Hairline() }
    }
}

/// The top line of a pushed page: back, and what it is on the right.
struct PageTop<Trailing: View>: View {
    let back: String
    /// Writes where back goes beside the chevron, "‹ Changes".
    var labelled = false
    @ViewBuilder var trailing: Trailing
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        HStack(spacing: 8) {
            Button { dismiss() } label: {
                HStack(spacing: 2) {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 19, weight: .medium))
                        .frame(width: labelled ? 24 : 44, height: 44)
                    if labelled { Text(back).font(Face.geist(16, .medium)) }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .foregroundStyle(labelled ? Ink.ac : Ink.ink)
            .accessibilityLabel(back)
            .accessibilityIdentifier("back")
            Spacer(minLength: 0)
            trailing
        }
        .padding(.leading, 8)
        .padding(.trailing, 18)
        .frame(height: 44)
    }
}

extension PageTop where Trailing == Text {
    init(back: String, meta: String?) {
        self.back = back
        trailing = Text(meta ?? "").font(Face.meta).foregroundStyle(Ink.mut)
    }
}

/// A page in the Inbox design: its own top line, no system bar, the near-white ground.
struct InboxPage: ViewModifier {
    func body(content: Content) -> some View {
        content
            .background(Ink.bg.ignoresSafeArea())
            .toolbar(.hidden, for: .navigationBar)
    }
}

extension View {
    func inboxPage() -> some View { modifier(InboxPage()) }
}

/// A destructive choice as a sheet: what each does, then its buttons. Cancel is always last.
struct ChoiceSheet: View {
    struct Choice: Identifiable {
        let title: String
        var detail: String?
        var destructive = true
        var id: String { title }
        let action: () -> Void
    }

    let title: String
    let choices: [Choice]
    var explained = true
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(title)
                .font(Face.h1)
                .tracking(-0.84)
                .foregroundStyle(Ink.ink)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            if explained {
                ForEach(Array(choices.enumerated()), id: \.element.id) { i, c in
                    if let d = c.detail {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(c.title).font(Face.title).foregroundStyle(Ink.ink)
                            Text(d).font(Face.sub).foregroundStyle(Ink.sub).fixedSize(horizontal: false, vertical: true)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.vertical, 12)
                        .overlay(alignment: .top) { if i > 0 { Hairline() } }
                        .padding(.top, i == 0 ? 10 : 0)
                    }
                }
            }
            VStack(spacing: 10) {
                ForEach(choices) { c in
                    Button(c.title) { dismiss(); c.action() }
                        .buttonStyle(BigButtonStyle(kind: .outline, destructive: c.destructive))
                        .accessibilityIdentifier("choice-\(c.title)")
                }
                Button("Cancel") { dismiss() }
                    .buttonStyle(BigButtonStyle(kind: .plain))
                    .foregroundStyle(Ink.sub)
            }
            .padding(.top, 14)
        }
        .padding(.horizontal, 20)
        .padding(.top, 14)
        .padding(.bottom, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Ink.sf)
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationCornerRadius(28)
        .presentationBackground(Ink.sf)
    }
}

/// A row that says something is loading or failed, in a list's place.
struct StateRow: View {
    let text: String
    var busy = false
    var error = false

    var body: some View {
        HStack(spacing: 10) {
            if busy { ProgressView().controlSize(.small) }
            Text(text)
                .font(Face.sub)
                .foregroundStyle(error ? Ink.bad : Ink.mut)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(.vertical, 14)
        .frame(minHeight: 52)
    }
}

/// Grey rows in a list's place before its first answer: the shape of what's coming, never made-up names.
struct LoadingRows: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var dim = false
    private static let widths: [(CGFloat, CGFloat)] = [(0.42, 0.7), (0.55, 0.48), (0.36, 0.62), (0.48, 0.4)]

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Self.widths.indices, id: \.self) { i in
                let (title, sub) = Self.widths[i]
                HStack(spacing: 12) {
                    Circle().fill(Ink.fill).frame(width: 12, height: 12)
                    GeometryReader { g in
                        VStack(alignment: .leading, spacing: 8) {
                            Capsule().fill(Ink.fill).frame(width: g.size.width * title, height: 13)
                            Capsule().fill(Ink.fill).frame(width: g.size.width * sub, height: 10)
                        }
                        .frame(maxHeight: .infinity, alignment: .center)
                    }
                }
                .padding(.vertical, 12)
                .frame(height: 60)
                .overlay(alignment: .top) { if i > 0 { Hairline() } }
            }
        }
        .opacity(dim ? 0.4 : 1)
        .animation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true), value: dim)
        .onAppear { dim = !reduceMotion }
        .accessibilityElement()
        .accessibilityLabel("Loading")
        .accessibilityIdentifier("loadingRows")
    }
}
