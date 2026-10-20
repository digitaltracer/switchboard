import CryptoKit
import Foundation
import LocalAuthentication
import Security

/// A device has two P-256 keys: `auth` signs token challenges without asking, `approval` signs
/// approvals and needs Face ID, Touch ID or the passcode for every use.
public enum KeyRole: String, Sendable, CaseIterable {
    case auth
    case approval
}

public protocol DeviceKeys: Sendable {
    /// SEC1 uncompressed (X9.63) public key.
    func publicKey(_ role: KeyRole) throws -> Data
    /// DER ECDSA over SHA-256 of `message`. For `approval` the system asks for the user first.
    func sign(_ message: String, with role: KeyRole, reason: String) async throws -> Data
    /// Forgets both keys, e.g. when re-pairing.
    func reset() throws
}

public enum KeyError: Error, LocalizedError {
    case keychain(OSStatus)
    case accessControl
    case userCancelled
    case noPasscode

    public var errorDescription: String? {
        switch self {
        case .keychain(let s): return "Keychain error \(s)"
        case .accessControl: return "Couldn't set up the key's access control"
        case .userCancelled: return "Cancelled"
        case .noPasscode: return "Set a passcode on this phone first: approvals need Face ID or the passcode."
        }
    }
}

/// In-memory software keys, for tests.
public final class SoftwareKeys: DeviceKeys, @unchecked Sendable {
    private var keys: [KeyRole: P256.Signing.PrivateKey] = [:]
    private let lock = NSLock()

    public init() {}

    private func key(_ role: KeyRole) -> P256.Signing.PrivateKey {
        lock.lock()
        defer { lock.unlock() }
        if let k = keys[role] { return k }
        let k = P256.Signing.PrivateKey()
        keys[role] = k
        return k
    }

    public func publicKey(_ role: KeyRole) throws -> Data {
        key(role).publicKey.x963Representation
    }

    public func sign(_ message: String, with role: KeyRole, reason: String) async throws -> Data {
        try key(role).signature(for: Data(message.utf8)).derRepresentation
    }

    public func reset() throws {
        lock.lock()
        keys = [:]
        lock.unlock()
    }
}

/// Keys in the Secure Enclave, kept as the enclave's opaque blobs in the keychain. The approval
/// key's access control is `.userPresence`: biometry or the passcode, not "current biometric
/// set", so enrolling a new finger doesn't lock you out. Where there is no Secure Enclave (the
/// simulator) it falls back to software keys in the keychain, with the same user check done by
/// LocalAuthentication before signing; that fallback is for development only.
public final class SecureEnclaveKeys: DeviceKeys, @unchecked Sendable {
    private let service: String
    private let lock = NSLock()

    public init(service: String = "switchboard.device-keys") {
        self.service = service
    }

    /// The simulator can report an enclave, but has no passcode for the approval key's policy.
    public static var hardwareBacked: Bool {
        #if targetEnvironment(simulator)
        false
        #else
        SecureEnclave.isAvailable
        #endif
    }

    public func publicKey(_ role: KeyRole) throws -> Data {
        if Self.hardwareBacked {
            return try enclaveKey(role, context: nil).publicKey.x963Representation
        }
        return try softwareKey(role).publicKey.x963Representation
    }

    public func sign(_ message: String, with role: KeyRole, reason: String) async throws -> Data {
        let data = Data(message.utf8)
        if Self.hardwareBacked {
            let context = LAContext()
            context.localizedReason = reason
            return try enclaveKey(role, context: context).signature(for: data).derRepresentation
        }
        if role == .approval {
            try await Self.confirmUser(reason: reason)
        }
        return try softwareKey(role).signature(for: data).derRepresentation
    }

    public func reset() throws {
        for role in KeyRole.allCases {
            let q: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: service,
                kSecAttrAccount as String: role.rawValue,
            ]
            let s = SecItemDelete(q as CFDictionary)
            guard s == errSecSuccess || s == errSecItemNotFound else { throw KeyError.keychain(s) }
        }
    }

    private static func confirmUser(reason: String) async throws {
        let context = LAContext()
        do {
            guard try await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason) else {
                throw KeyError.userCancelled
            }
        } catch {
            throw KeyError.userCancelled
        }
    }

    private func enclaveKey(_ role: KeyRole, context: LAContext?) throws -> SecureEnclave.P256.Signing.PrivateKey {
        lock.lock()
        defer { lock.unlock() }
        if let blob = try load(role) {
            return try SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: blob, authenticationContext: context)
        }
        let flags: SecAccessControlCreateFlags = role == .approval ? [.privateKeyUsage, .userPresence] : [.privateKeyUsage]
        let protection = role == .approval
            ? kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly
            : kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        guard let access = SecAccessControlCreateWithFlags(nil, protection, flags, nil) else {
            throw KeyError.accessControl
        }
        let key: SecureEnclave.P256.Signing.PrivateKey
        do {
            key = try SecureEnclave.P256.Signing.PrivateKey(accessControl: access, authenticationContext: context)
        } catch {
            throw KeyError.noPasscode
        }
        try store(role, key.dataRepresentation)
        return key
    }

    private func softwareKey(_ role: KeyRole) throws -> P256.Signing.PrivateKey {
        lock.lock()
        defer { lock.unlock() }
        if let raw = try load(role) {
            return try P256.Signing.PrivateKey(rawRepresentation: raw)
        }
        let key = P256.Signing.PrivateKey()
        try store(role, key.rawRepresentation)
        return key
    }

    private func load(_ role: KeyRole) throws -> Data? {
        let q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: role.rawValue,
            kSecReturnData as String: true,
        ]
        var out: CFTypeRef?
        let s = SecItemCopyMatching(q as CFDictionary, &out)
        if s == errSecItemNotFound { return nil }
        guard s == errSecSuccess else { throw KeyError.keychain(s) }
        return out as? Data
    }

    private func store(_ role: KeyRole, _ data: Data) throws {
        let q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: role.rawValue,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
            kSecValueData as String: data,
        ]
        let s = SecItemAdd(q as CFDictionary, nil)
        guard s == errSecSuccess else { throw KeyError.keychain(s) }
    }
}
