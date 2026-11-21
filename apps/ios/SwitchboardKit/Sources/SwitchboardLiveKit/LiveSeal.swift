import CryptoKit
import Foundation

extension Data {
    public init?(base64URL: String) {
        var s = base64URL.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        s += String(repeating: "=", count: (4 - s.count % 4) % 4)
        self.init(base64Encoded: s)
    }

    public var base64URL: String {
        base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }
}

/// Opens a board the hub sealed to this device, as `src/live.rs` seals it: ECDH between the
/// hub's one-off key and the live key, HKDF-SHA256 salted with both public keys, ChaCha20-Poly1305.
public enum LiveSeal {
    public enum Failure: Error { case malformed, open }

    public static func open(_ blob: String, with key: P256.KeyAgreement.PrivateKey) throws -> Data {
        guard let raw = Data(base64URL: blob), raw.count >= 65 + 12 + 16 else { throw Failure.malformed }
        let epk = raw.prefix(65)
        let nonce = raw.dropFirst(65).prefix(12)
        let ct = raw.dropFirst(77)
        guard let eph = try? P256.KeyAgreement.PublicKey(x963Representation: epk) else { throw Failure.malformed }
        let shared = try key.sharedSecretFromKeyAgreement(with: eph)
        let sym = shared.hkdfDerivedSymmetricKey(
            using: SHA256.self, salt: Data(epk) + key.publicKey.x963Representation,
            sharedInfo: Data("switchboard-live-v1".utf8), outputByteCount: 32
        )
        do {
            let box = try ChaChaPoly.SealedBox(nonce: ChaChaPoly.Nonce(data: nonce), ciphertext: ct.dropLast(16), tag: ct.suffix(16))
            return try ChaChaPoly.open(box, using: sym)
        } catch {
            throw Failure.open
        }
    }

    public static func board(_ blob: String, with key: P256.KeyAgreement.PrivateKey) throws -> LiveBoard {
        try JSONDecoder().decode(LiveBoard.self, from: open(blob, with: key))
    }
}

extension LiveSeal {
    /// Seals as the hub does; the app uses it for its own local updates.
    public static func seal(_ plain: Data, to device: P256.KeyAgreement.PublicKey) throws -> String {
        let eph = P256.KeyAgreement.PrivateKey()
        let shared = try eph.sharedSecretFromKeyAgreement(with: device)
        let epk = eph.publicKey.x963Representation
        let sym = shared.hkdfDerivedSymmetricKey(
            using: SHA256.self, salt: epk + device.x963Representation,
            sharedInfo: Data("switchboard-live-v1".utf8), outputByteCount: 32
        )
        let box = try ChaChaPoly.seal(plain, using: sym)
        return (epk + Data(box.nonce) + box.ciphertext + box.tag).base64URL
    }
}
