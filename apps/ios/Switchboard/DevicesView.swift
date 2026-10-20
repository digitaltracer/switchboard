import SwiftUI
import SwitchboardKit

/// Paired devices. Revoking one refuses its next request; to replace a lost phone, pair the new
/// one first, then revoke the old one here.
struct DevicesView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var devices: [Schemas.Device] = []
    @State private var error: String?
    @State private var revoking: Schemas.Device?
    @State private var confirmForget = false

    var body: some View {
        NavigationStack {
            List {
                if let error { Text(error).foregroundStyle(.red) }
                Section("Paired devices") {
                    ForEach(devices, id: \.id) { d in
                        VStack(alignment: .leading, spacing: 2) {
                            HStack {
                                Text(d.name).font(.headline)
                                if d.thisDevice { Text("this phone").font(.caption).foregroundStyle(.secondary) }
                                Spacer()
                                if d.state != "active" { Text(d.state).font(.caption).foregroundStyle(.orange) }
                            }
                            Text(d.fingerprint).font(.caption.monospaced()).foregroundStyle(.secondary)
                            if !d.pushEnabled { Text("No push").font(.caption2).foregroundStyle(.secondary) }
                        }
                        .swipeActions {
                            Button("Revoke", role: .destructive) { revoking = d }
                        }
                    }
                }
                if let p = model.pairing {
                    Section("Hub") {
                        LabeledContent("Address", value: p.hubURL.absoluteString)
                        if let m = model.meta { LabeledContent("Oldest supported build", value: "\(m.minClientBuild)") }
                        Button("Forget this hub", role: .destructive) { confirmForget = true }
                    }
                }
            }
            .navigationTitle("Devices")
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .task { await load() }
            .refreshable { await load() }
            .confirmationDialog("Revoke \(revoking?.name ?? "")? Its next request is refused.",
                                isPresented: Binding(get: { revoking != nil }, set: { if !$0 { revoking = nil } })) {
                Button("Revoke", role: .destructive) { Task { await revoke() } }
            }
            .confirmationDialog("Forget this hub? This phone's keys are deleted; revoke it at the hub too.", isPresented: $confirmForget) {
                Button("Forget", role: .destructive) { model.forget() }
            }
        }
    }

    private func load() async {
        guard let hub = model.hub else { return }
        do { devices = try await hub.devices() } catch { self.error = error.localizedDescription }
    }

    private func revoke() async {
        guard let hub = model.hub, let d = revoking else { return }
        revoking = nil
        do {
            try await hub.revoke(d.id)
            if d.thisDevice { model.forget() } else { await load() }
        } catch {
            self.error = error.localizedDescription
        }
    }
}
