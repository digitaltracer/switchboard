import SwiftUI
import SwitchboardKit
import VisionKit

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        switch model.phase {
        case .paired:
            HomeView()
        case .unpaired, .confirming:
            PairingView()
        }
    }
}

/// Scan or paste the link from `switchboard admin pair-device`, then compare fingerprints.
struct PairingView: View {
    @Environment(AppModel.self) private var model
    @State private var linkText = ProcessInfo.processInfo.environment["SB_PAIR_LINK"] ?? ""
    @State private var name = "iPhone"
    @State private var scanning = false
    @State private var working = false
    @State private var pairTask: Task<Void, Never>?
    @State private var cameraNote: String?

    var body: some View {
        NavigationStack {
            Form {
                switch model.phase {
                case .confirming(let fingerprint):
                    Section("Confirm on your computer") {
                        Text("Check that the terminal running pair-device shows exactly this fingerprint, then type yes there.")
                        Text(fingerprint)
                            .font(.title3.monospaced())
                            .accessibilityIdentifier("fingerprint")
                        ProgressView("Waiting for the hub…")
                        Button("Cancel", role: .cancel) { pairTask?.cancel() }
                            .accessibilityIdentifier("cancelPairing")
                    }
                default:
                    Section {
                        Text("On the hub's machine run `switchboard admin pair-device`, then scan its QR code or paste the link.")
                            .font(.callout)
                        if DataScannerViewController.isSupported {
                            Button("Scan QR code") {
                                if DataScannerViewController.isAvailable {
                                    scanning = true
                                } else {
                                    cameraNote = "Switchboard can't use the camera. Allow it in Settings, or paste the link."
                                }
                            }
                            if let cameraNote { Text(cameraNote).font(.caption).foregroundStyle(.orange) }
                        }
                        TextField("switchboard://pair?…", text: $linkText, axis: .vertical)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .font(.footnote.monospaced())
                            .accessibilityIdentifier("pairLink")
                        TextField("This phone's name", text: $name)
                            .accessibilityIdentifier("deviceName")
                    }
                    if let error = model.pairingError {
                        Section { Text(error).foregroundStyle(.red) }
                    }
                    Section {
                        Button("Pair") { pair() }
                            .disabled(PairingLink(linkText) == nil || name.trimmingCharacters(in: .whitespaces).isEmpty || working)
                            .accessibilityIdentifier("pairButton")
                    }
                }
            }
            .navigationTitle("Pair with your hub")
            .sheet(isPresented: $scanning) {
                QRScanner { code in
                    linkText = code
                    scanning = false
                }
                .ignoresSafeArea()
            }
            .onOpenURL { url in linkText = url.absoluteString }
        }
    }

    private func pair() {
        guard let link = PairingLink(linkText) else { return }
        working = true
        pairTask = Task {
            await model.pair(link: link, name: name.trimmingCharacters(in: .whitespaces))
            working = false
        }
    }
}

/// The camera, looking for a Switchboard pairing QR code.
struct QRScanner: UIViewControllerRepresentable {
    let found: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let vc = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])], isHighlightingEnabled: true)
        vc.delegate = context.coordinator
        try? vc.startScanning()
        return vc
    }

    func updateUIViewController(_ vc: DataScannerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(found: found) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let found: (String) -> Void
        init(found: @escaping (String) -> Void) { self.found = found }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for case .barcode(let b) in items {
                if let s = b.payloadStringValue, PairingLink(s) != nil {
                    scanner.stopScanning()
                    found(s)
                    return
                }
            }
        }
    }
}
