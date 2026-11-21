import SwiftUI
import SwitchboardLiveKit

/// Where each body of the card's solar system goes. Bodies sit on tilted elliptical orbits around
/// the lead, so any number of agents fits; positions are in points on a 150 pt tall card.
struct LiveLayout {
    struct Body: Identifiable {
        enum Kind { case planet(String), moon }
        let id: String
        let kind: Kind
        let at: CGPoint
        let radius: CGFloat
        let label: String?
    }

    static let tilt = 0.5
    static let rotation = -10.0 * .pi / 180

    let centre: CGPoint
    let rings: [CGFloat]
    let bodies: [Body]

    static func point(_ c: CGPoint, _ a: CGFloat, _ deg: Double) -> CGPoint {
        let t = deg * .pi / 180
        let x = a * cos(t), y = a * tilt * sin(t)
        return CGPoint(x: c.x + x * cos(rotation) - y * sin(rotation), y: c.y + x * sin(rotation) + y * cos(rotation))
    }

    /// Orbit slots: (ring, degrees). Named planets take them first; moons go outside.
    static let leadSlots: [(Int, Double)] = [(0, 35), (1, -48), (2, 8), (1, 80)]
    static let twoSlots: [(Int, Double)] = [(1, 30), (2, -22), (2, 62)]
    // Bodies and labels stay left of the text column, so a moon never reads as a legend icon.
    static let calmSlots: [(Int, Double)] = [(0, -80), (0, 125), (1, -140), (1, -30), (1, 62)]
    static let leadMoons: [(Int, Double)] = [(3, -35), (3, 52), (3, 78), (3, -56), (2, 40)]
    static let twoMoons: [(Int, Double)] = [(2, 95), (2, -60), (3, 110), (3, -30)]
    static let calmMoons: [(Int, Double)] = [(2, -45), (2, 100), (2, -100), (2, 140)]

    static let textures = ["jupiter", "saturn", "earth", "mars", "neptune", "venus"]
    static let radii: [String: CGFloat] = ["jupiter": 8, "saturn": 6.5, "earth": 6, "mars": 5, "neptune": 7, "venus": 5.5]

    /// The same agent keeps its planet from update to update.
    static func texture(_ name: String, avoiding used: Set<String> = []) -> String {
        var h: UInt32 = 5381
        for b in name.utf8 { h = (h &* 33) &+ UInt32(b) }
        let start = Int(h % UInt32(textures.count))
        let order = (0..<textures.count).map { textures[(start + $0) % textures.count] }
        return order.first { !used.contains($0) } ?? order[0]
    }

    init(_ board: LiveBoard, mood: LiveMood) {
        let slots: [(Int, Double)], moons: [(Int, Double)]
        switch mood {
        case .calm:
            centre = CGPoint(x: 86, y: 72)
            rings = [46, 72, 104]
            (slots, moons) = (Self.calmSlots, Self.calmMoons)
        case .needsYou(_, let others) where others > 0:
            centre = CGPoint(x: 62, y: 68)
            rings = [74, 98, 122, 140]
            (slots, moons) = (Self.twoSlots, Self.twoMoons)
        default:
            centre = CGPoint(x: 50, y: 75)
            rings = [66, 90, 114, 140]
            (slots, moons) = (Self.leadSlots, Self.leadMoons)
        }
        var out: [Body] = []
        // A finished agent is the lead, so it isn't also a planet.
        var names = board.running
        if case .finished(let f) = mood { names.removeAll { $0 == f } }
        var used = Set<String>()
        for (i, name) in names.prefix(slots.count).enumerated() {
            let (ring, deg) = slots[i]
            let tex = Self.texture(name, avoiding: used)
            used.insert(tex)
            out.append(Body(id: "p-\(name)", kind: .planet(tex), at: Self.point(centre, rings[ring], deg),
                            radius: Self.radii[tex] ?? 6, label: Self.short(name)))
        }
        // Running agents beyond the labelled ones are small, unnamed planets.
        let extra = max(0, board.runningCount - min(names.count, slots.count))
        let free = Array(moons.dropFirst(min(board.idle, moons.count)))
        for i in 0..<min(extra, free.count) {
            let (ring, deg) = free[i]
            out.append(Body(id: "x-\(i)", kind: .planet(Self.textures[i % Self.textures.count]),
                            at: Self.point(centre, rings[min(ring, rings.count - 1)], deg), radius: 4, label: nil))
        }
        for i in 0..<min(board.idle, moons.count) {
            let (ring, deg) = moons[i]
            out.append(Body(id: "m-\(i)", kind: .moon, at: Self.point(centre, rings[min(ring, rings.count - 1)], deg),
                            radius: i % 2 == 0 ? 3.2 : 2.8, label: nil))
        }
        bodies = out
    }

    static func short(_ name: String) -> String {
        name.count <= 12 ? name : String(name.prefix(11)) + "…"
    }
}

/// The lead of the system: what the card is about.
enum LiveLead: Equatable {
    case sun, redSun, finished, none

    init(_ mood: LiveMood) {
        switch mood {
        case .needsYou: self = .sun
        case .error: self = .redSun
        case .finished: self = .finished
        case .calm: self = .none
        }
    }

    var image: String? {
        switch self {
        case .sun: "live-sun"
        case .redSun: "live-sun-red"
        case .finished: "live-finished"
        case .none: nil
        }
    }

    var glow: Color {
        switch self {
        case .sun: Color(red: 1, green: 0.55, blue: 0.22)
        case .redSun: Color(red: 1, green: 0.25, blue: 0.16)
        case .finished: Color(red: 0.30, green: 0.92, blue: 0.58)
        case .none: .clear
        }
    }
}

/// A sprite whose disc is a third of its image, sized by the disc's radius.
struct LiveSprite: View {
    let name: String
    let radius: CGFloat

    var body: some View {
        Image(name).resizable().interpolation(.high).frame(width: radius * 6, height: radius * 6)
    }
}

struct LiveScene: View {
    let board: LiveBoard
    let mood: LiveMood
    /// Where the text column starts; orbits fade out before it.
    let fade: CGFloat

    var body: some View {
        let layout = LiveLayout(board, mood: mood)
        let lead = LiveLead(mood)
        ZStack(alignment: .topLeading) {
            if lead != .none {
                RadialGradient(colors: [lead.glow.opacity(0.22), .clear], center: .init(x: layout.centre.x / 366, y: 0.5),
                               startRadius: 0, endRadius: 150)
            }
            LiveOrbits(centre: layout.centre, rings: layout.rings)
                .mask(LinearGradient(stops: [.init(color: .black, location: 0), .init(color: .black, location: max(0, (fade - 40) / 366)),
                                             .init(color: .clear, location: min(1, (fade + 6) / 366))],
                                     startPoint: .leading, endPoint: .trailing))
            ForEach(layout.bodies) { b in
                Group {
                    switch b.kind {
                    case .planet(let tex): LiveSprite(name: "live-\(tex)", radius: b.radius)
                    case .moon: Image("live-moon").resizable().frame(width: b.radius / 0.32, height: b.radius / 0.32)
                    }
                }
                .position(b.at)
                if let label = b.label {
                    Text(label)
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(Color(red: 0.79, green: 0.80, blue: 0.82))
                        .shadow(color: .black.opacity(0.9), radius: 1.5, y: 1)
                        .fixedSize()
                        .position(x: b.at.x, y: b.at.y + b.radius + 9)
                }
            }
            LiveLeadBodies(board: board, mood: mood, layout: layout)
        }
        .frame(width: 366, height: 150, alignment: .topLeading)
    }
}

/// The lead, which "ignites" once when the state changes.
struct LiveLeadBodies: View {
    let board: LiveBoard
    let mood: LiveMood
    let layout: LiveLayout

    var body: some View {
        let lead = LiveLead(mood)
        ZStack(alignment: .topLeading) {
            switch mood {
            case .needsYou(_, let others) where others > 0:
                LiveSprite(name: "live-sun", radius: 27).position(x: 44, y: 80)
                LiveSprite(name: board.waiting.count > 1 && board.waiting[1].error ? "live-sun-red" : "live-sun", radius: 15)
                    .position(x: 110, y: 36)
                if board.waiting.count > 1 {
                    let w = board.waiting[1]
                    HStack(spacing: 0) {
                        Text(LiveLayout.short(w.name) + " · ")
                        LiveClock(since: w.since, width: 32)
                    }
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(LiveCard.amber)
                    .shadow(color: .black.opacity(0.9), radius: 1.5, y: 1)
                    .frame(width: 130)
                    .position(x: 112, y: 61)
                }
            case .finished:
                LiveSprite(name: "live-finished", radius: 16).position(layout.centre)
            default:
                if let image = lead.image {
                    LiveSprite(name: image, radius: lead == .redSun ? 27 : 29).position(layout.centre)
                }
            }
        }
        .id(LiveCard.key(board, mood))
        .transition(.asymmetric(insertion: .scale(scale: 0.35).combined(with: .opacity), removal: .opacity))
    }
}

/// Plain shapes rather than a Canvas, which a Live Activity may not draw.
struct LiveOrbits: View {
    let centre: CGPoint
    let rings: [CGFloat]

    var body: some View {
        ZStack(alignment: .topLeading) {
            ForEach(rings, id: \.self) { a in
                Ellipse()
                    .stroke(Color(red: 0.72, green: 0.80, blue: 1).opacity(0.19), lineWidth: 0.6)
                    .frame(width: 2 * a, height: 2 * a * LiveLayout.tilt)
                    .rotationEffect(.radians(LiveLayout.rotation))
                    .position(centre)
            }
        }
    }
}
