import SwiftUI
import SwitchboardKit

/// Light, dark or the phone's own setting, kept on the phone.
enum Appearance: String, CaseIterable, Identifiable {
    case system, light, dark

    static let key = "appearance"

    var id: Self { self }
    var label: String { rawValue.capitalized }
    var scheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }
}

/// Paired devices, appearance, voice and the hub. Revoking a device refuses its next request; to
/// replace a lost phone, pair the new one first, then revoke the old one here.
struct DevicesView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @AppStorage(Appearance.key) private var appearance = Appearance.system
    @State private var devices: [Schemas.Device]?
    @State private var error: String?
    @State private var revoking: Schemas.Device?
    @State private var confirmForget = false
    @State private var newVoiceKey = ""
    @State private var voiceKeyError: String?
    @State private var shipSettings = false

    var body: some View {
        content.sheet(isPresented: $shipSettings) { ShipSettingsView() }
    }

    private var content: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Settings").font(Face.h1).tracking(-0.84).foregroundStyle(Ink.ink)
                Spacer()
                Button("Done") { dismiss() }
                    .font(Face.geist(16, .medium))
                    .foregroundStyle(Ink.ac)
                    .frame(minHeight: 44)
            }
            .padding(.horizontal, 20)
            .padding(.top, 14)
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    if let error { Note(text: error, color: Ink.bad).padding(.top, 12) }
                    SectionHead("Paired devices", count: devices?.count).padding(.top, -6)
                    if devices == nil && error == nil { StateRow(text: "Listing devices…", busy: true) }
                    ForEach(Array((devices ?? []).enumerated()), id: \.element.id) { i, d in device(d, first: i == 0) }
                    Hairline()
                    SectionHead("Appearance")
                    HStack(spacing: 8) {
                        ForEach(Appearance.allCases) { a in
                            Button { appearance = a } label: {
                                Text(a.label)
                                    .font(Face.geist(15, .medium))
                                    .foregroundStyle(appearance == a ? Ink.inv : Ink.sub)
                                    .frame(maxWidth: .infinity, minHeight: 44)
                                    .background(appearance == a ? Ink.ink : Ink.fill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                            }
                            .buttonStyle(.plain)
                            .accessibilityAddTraits(appearance == a ? .isSelected : [])
                            .accessibilityIdentifier("appearance-\(a.rawValue)")
                        }
                    }
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("appearancePicker")
                    voiceSection
                    if model.pairing != nil {
                        SectionHead("Ship actions")
                        Button { shipSettings = true } label: {
                            Row(title: "Buttons on a finished session", leading: { EmptyView() }, trailing: { Chevron() })
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("shipSettings")
                        AwaySettingsSection()
                    }
                    LiveActivityLab()
                    if let p = model.pairing {
                        SectionHead("Hub")
                        FieldRow(p.relay == nil ? "Address" : "Relay", value: p.hubURL.host() ?? p.hubURL.absoluteString, mono: true, first: true)
                        if let m = model.meta { FieldRow("Oldest supported build", value: "\(m.minClientBuild)") }
                        Button { confirmForget = true } label: {
                            Text("Forget this hub").font(Face.geist(16, .medium)).foregroundStyle(Ink.bad)
                                .frame(maxWidth: .infinity, minHeight: 48, alignment: .leading)
                                .overlay(alignment: .top) { Hairline() }
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("forgetHub")
                        Hairline()
                    }
                }
                .padding(.horizontal, 20)
                .padding(.bottom, 32)
            }
            .refreshable { await load() }
        }
        .background(Ink.bg.ignoresSafeArea())
        .presentationBackground(Ink.bg)
        .task { await load() }
        .sheet(item: $revoking) { d in
            ChoiceSheet(title: "Revoke \(d.name)?", choices: [
                .init(title: "Revoke", detail: d.thisDevice
                    ? "This phone's next request is refused and it forgets the hub. Pair it again to use it."
                    : "Its next request is refused. It stays paired until then, and can't be un-revoked.") {
                    Task { await revoke(d) }
                },
            ])
        }
        .sheet(isPresented: $confirmForget) {
            ChoiceSheet(title: "Forget this hub?", choices: [
                .init(title: "Forget", detail: "This phone's keys are deleted and the hub stops pushing to it. Revoke it at the hub too.") {
                    Task { await model.unpair() }
                },
            ])
        }
    }

    private func device(_ d: Schemas.Device, first: Bool) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 8) {
                    Text(d.name).font(Face.title).foregroundStyle(Ink.ink)
                    if d.thisDevice { Chip(text: "this phone").scaleEffect(0.86, anchor: .leading) }
                    if d.state != "active" { Text(d.state).font(Face.meta).foregroundStyle(Ink.warn) }
                }
                Text(d.fingerprint).font(Face.mono(12.5)).foregroundStyle(Ink.sub).lineLimit(1)
                if !d.pushEnabled { Text("No push").font(Face.meta).foregroundStyle(Ink.mut) }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button("Revoke") { revoking = d }
                .font(Face.geist(15, .medium))
                .foregroundStyle(Ink.bad)
                .frame(minHeight: 44)
                .accessibilityIdentifier("revoke-\(d.name)")
        }
        .padding(.vertical, 12)
        .frame(minHeight: 60)
        .overlay(alignment: .top) { if !first { Hairline() } }
    }

    /// An OpenAI key, kept on this phone. Once saved it's never shown again.
    @ViewBuilder
    private var voiceSection: some View {
        SectionHead(title: "Voice") { Text("On this phone") }
        if model.voiceKey.saved {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) { savedKey; removeKey.frame(width: 130) }
                VStack(spacing: 8) { savedKey; removeKey }
            }
        } else {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 8) { keyField; saveKey.fixedSize() }
                VStack(spacing: 8) { keyField; saveKey }
            }
        }
        if let voiceKeyError { Note(text: voiceKeyError, color: Ink.bad).padding(.top, 8) }
        Note(text: VoiceKey.disclosure).padding(.top, 8).onAppear { model.voiceKey.refresh() }
    }

    private var savedKey: some View {
        Text("Key saved").font(Face.geist(16)).foregroundStyle(Ink.ink)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 16)
            .frame(minHeight: 52)
            .background(Ink.sf, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Ink.line))
            .accessibilityIdentifier("voiceKeySaved")
    }

    private var removeKey: some View {
        Button("Remove key") { model.voiceKey.remove() }
            .buttonStyle(BigButtonStyle(kind: .outline, destructive: true))
            .accessibilityIdentifier("voiceKeyRemove")
    }

    private var keyField: some View {
        SecureField("OpenAI API key", text: $newVoiceKey)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .font(Face.geist(16))
            .padding(.horizontal, 16)
            .frame(minHeight: 52)
            .background(Ink.sf, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Ink.line))
            .accessibilityIdentifier("voiceKeyField")
    }

    private var saveKey: some View {
        Button("Save key") {
            voiceKeyError = model.voiceKey.save(newVoiceKey) ? nil : "This phone couldn't store the key, so voice stays on the device."
            newVoiceKey = ""
        }
        .buttonStyle(BigButtonStyle())
        .disabled(newVoiceKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        .accessibilityIdentifier("voiceKeySave")
    }

    private func load() async {
        guard let hub = model.hub else { return }
        do { devices = try await hub.devices(); error = nil } catch { self.error = error.localizedDescription }
    }

    private func revoke(_ d: Schemas.Device) async {
        guard let hub = model.hub else { return }
        revoking = nil
        do {
            try await hub.revoke(d.id)
            if d.thisDevice { model.forget() } else { await load() }
        } catch {
            self.error = error.localizedDescription
        }
    }
}

extension Schemas.Device: @retroactive Identifiable {}
