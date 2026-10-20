import CryptoKit
import Foundation

/// The hub's canonical signed messages; `src/auth/device.rs` builds the same strings.
public enum SignedMessage {
    public static let version: Int32 = 1

    public static func token(hub: String, device: String, nonce: String, expiresAt: Int64) -> String {
        [
            "switchboard-token-v\(version)",
            "hub=\(hub)",
            "device=\(device)",
            "nonce=\(nonce)",
            "expires=\(expiresAt)",
        ].joined(separator: "\n")
    }

    /// Only approvals are signed; a deny needs just the device token.
    public static func approval(hub: String, device: String, approval: String, nonce: String, expiresAt: Int64) -> String {
        [
            "switchboard-approval-v\(version)",
            "hub=\(hub)",
            "device=\(device)",
            "approval=\(approval)",
            "decision=allow",
            "nonce=\(nonce)",
            "expires=\(expiresAt)",
        ].joined(separator: "\n")
    }
}

/// What the phone shows and the admin's terminal must match: SHA-256 over the hub id and both
/// public keys, as six groups of four hex digits.
public func pairingFingerprint(hubId: String, authKey: String, approvalKey: String) -> String {
    let digest = SHA256.hash(data: Data("\(hubId)\n\(authKey)\n\(approvalKey)".utf8))
    let hex = digest.prefix(12).map { String(format: "%02x", $0) }.joined()
    return stride(from: 0, to: hex.count, by: 4).map { i in
        let start = hex.index(hex.startIndex, offsetBy: i)
        return String(hex[start..<hex.index(start, offsetBy: 4)])
    }.joined(separator: "-")
}

extension Data {
    public var base64URL: String {
        base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}
