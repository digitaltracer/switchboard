#if canImport(ActivityKit) && os(iOS)
import ActivityKit
import Foundation

/// The lock-screen card's activity. Its state is a sealed board and nothing readable: the relay
/// and Apple's push service carry it, so only this device's live key opens it.
public struct AgentsActivity: ActivityAttributes {
    public struct ContentState: Codable, Hashable, Sendable {
        public var v: Int
        /// `LiveSeal`'s blob of a `LiveBoard`.
        public var b: String

        public init(v: Int = 1, b: String) {
            self.v = v
            self.b = b
        }
    }

    public init() {}
}
#endif
