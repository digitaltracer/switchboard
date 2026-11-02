import Foundation
import Observation
import Security

/// This phone's OpenAI key, apart from its signing keys. It's unreadable while the phone is
/// locked, never leaves this device in a backup, and doesn't survive a reinstall. A key that
/// can't be read means voice stays on the device.
@MainActor
@Observable
final class VoiceKey {
    static let disclosure = "With a key saved, your voice recordings go to OpenAI to be transcribed, along with your project, branch, session and agent names so they come out spelled right. OpenAI bills this key. You still review the words before anything is sent. Without a key, speech is transcribed on this phone."

    private static let service = "switchboard.voice-provider"
    private static let account = "openai"
    private static let installSeen = "voiceKeyInstallSeen"

    private(set) var saved = false
    /// Told when the key is saved or removed, so voice can drop what depended on the old one.
    @ObservationIgnored var changed: ((_ removed: Bool) -> Void)?

    init() {
        // Keychain items can outlive the app, but this flag can't.
        if !UserDefaults.standard.bool(forKey: Self.installSeen), delete() {
            UserDefaults.standard.set(true, forKey: Self.installSeen)
        }
        refresh()
    }

    func refresh() {
        saved = read() != nil
    }

    /// The key itself, which is never shown and never kept in this object.
    func read() -> String? {
        var query = baseQuery
        query[kSecReturnData as String] = true
        var out: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &out) == errSecSuccess, let data = out as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    /// False if the phone wouldn't store it, and then nothing is kept anywhere else.
    @discardableResult
    func save(_ key: String) -> Bool {
        let trimmed = key.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, delete() else { return false }
        var query = baseQuery
        query[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        query[kSecValueData as String] = Data(trimmed.utf8)
        let stored = SecItemAdd(query as CFDictionary, nil) == errSecSuccess
        refresh()
        if stored { changed?(false) }
        return stored
    }

    func remove() {
        delete()
        refresh()
        changed?(true)
    }

    @discardableResult
    private func delete() -> Bool {
        let status = SecItemDelete(baseQuery as CFDictionary)
        return status == errSecSuccess || status == errSecItemNotFound
    }

    private var baseQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: Self.account,
        ]
    }
}
