import CryptoKit
import Foundation

/// The key boards are sealed to: a P-256 key-agreement key in a file in the App Group container
/// the app and its lock-screen extension share. On a phone, the process that draws Live
/// Activities can't reach the keychain (it gets errSecNotAvailable), so a file it is: mode 0600,
/// readable from the first unlock on so a locked phone can show the card, never backed up. The
/// app writes it; the extension only reads. It never signs anything; the device keys stay put.
public struct LiveKey: Sendable {
    public enum Failure: Error {
        case noContainer
        /// It can't be read now, e.g. before the first unlock.
        case unreadable(String)
        /// It reads but isn't a key: a write cut short.
        case corrupt
    }

    static let fileName = "live-key"

    /// The folder holding the file; `nil` when the App Group isn't available.
    let directory: URL?

    /// The App Group the app and extension both list, from the `SBAppGroup` Info.plist key.
    public static var shared: LiveKey {
        let group = Bundle.main.object(forInfoDictionaryKey: "SBAppGroup") as? String
        return LiveKey(directory: group.flatMap { FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: $0) })
    }

    public init(directory: URL?) {
        self.directory = directory
    }

    public var groupName: String? { directory?.lastPathComponent }

    private var file: URL? { directory?.appending(path: Self.fileName) }

    /// The key, made on first use. Only the app calls this.
    public func loadOrCreate() throws -> P256.KeyAgreement.PrivateKey {
        do {
            if let k = try load() { return k }
        } catch Failure.corrupt {
            // A file cut short by a crash mid-write: start over; the hub gets the new key.
            try? reset()
        }
        guard var file else { throw Failure.noContainer }
        let k = P256.KeyAgreement.PrivateKey()
        var options: Data.WritingOptions = [.withoutOverwriting]
        #if os(iOS)
        options.insert(.completeFileProtectionUntilFirstUserAuthentication)
        #endif
        do {
            try k.rawRepresentation.write(to: file, options: options)
        } catch CocoaError.fileWriteFileExists {
            if let again = try load() { return again }
            throw Failure.unreadable("the key file appeared but doesn't read")
        }
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try file.setResourceValues(values)
        return k
    }

    /// The key, or `nil` when the app hasn't made it yet.
    public func load() throws -> P256.KeyAgreement.PrivateKey? {
        guard let file else { throw Failure.noContainer }
        let data: Data
        do {
            data = try Data(contentsOf: file)
        } catch CocoaError.fileReadNoSuchFile {
            return nil
        } catch {
            throw Failure.unreadable("\(error)")
        }
        do {
            return try P256.KeyAgreement.PrivateKey(rawRepresentation: data)
        } catch {
            throw Failure.corrupt
        }
    }

    /// Forgets the key, e.g. when forgetting the hub.
    public func reset() throws {
        guard let file else { return }
        try? FileManager.default.removeItem(at: file)
    }
}
