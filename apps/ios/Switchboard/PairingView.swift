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
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Switchboard").font(Face.geist(13, .medium)).foregroundStyle(Ink.sub)
                Spacer()
                Text(confirming == nil ? "Step 1 of 2" : "Step 2 of 2").font(Face.meta).foregroundStyle(Ink.mut)
            }
            .padding(.horizontal, 20)
            .frame(height: 44)
            if let fingerprint = confirming { confirm(fingerprint) } else { form }
        }
        .background(Ink.bg.ignoresSafeArea())
        .sheet(isPresented: $scanning) {
            QRScanner { code in
                linkText = code
                scanning = false
            }
            .ignoresSafeArea()
        }
        .onOpenURL { url in linkText = url.absoluteString }
    }

    private var confirming: String? {
        if case .confirming(let f) = model.phase { f } else { nil }
    }

    private var form: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 10) {
                    Headline("Pair with your hub.").stateStyle().padding(.top, 28).accessibilityIdentifier("pairTitle")
                    (Text("On the hub’s machine run ") + Text("switchboard admin pair-device").font(Face.mono(14)).foregroundColor(Ink.ink)
                        + Text(", then scan the QR code it prints."))
                        .font(Face.geist(16)).lineSpacing(4).foregroundStyle(Ink.sub)
                        .fixedSize(horizontal: false, vertical: true)
                        .padding(.bottom, 18)
                    if DataScannerViewController.isSupported {
                        Button {
                            if DataScannerViewController.isAvailable {
                                scanning = true
                            } else {
                                cameraNote = "Switchboard can't use the camera. Allow it in Settings, or paste the link."
                            }
                        } label: { Label("Scan QR code", systemImage: "qrcode.viewfinder") }
                            .buttonStyle(BigButtonStyle())
                        if let cameraNote { Note(text: cameraNote, color: Ink.warn) }
                    }
                    SectionHead("Or paste the link").padding(.top, 8)
                    Text("Pairing link").font(Face.meta).foregroundStyle(Ink.mut)
                    TextField("switchboard://pair?…", text: $linkText, axis: .vertical)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .font(Face.mono(14))
                        .lineLimit(1...4)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 14)
                        .background(Ink.sf, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Ink.line))
                        .accessibilityIdentifier("pairLink")
                    Text("Name this phone").font(Face.meta).foregroundStyle(Ink.mut).padding(.top, 8)
                    TextField("This phone's name", text: $name)
                        .font(Face.geist(16))
                        .padding(.horizontal, 16)
                        .frame(minHeight: 52)
                        .background(Ink.sf, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Ink.line))
                        .accessibilityIdentifier("deviceName")
                    Note(text: "The name shows in the hub’s device list, so you can revoke this phone later.")
                    if let error = model.pairingError {
                        Banner(text: error).padding(.top, 6).accessibilityIdentifier("pairingError")
                    }
                }
                .padding(.horizontal, 20)
            }
            .scrollDismissesKeyboard(.interactively)
            VStack(spacing: 10) {
                Button("Pair") { pair() }
                    .buttonStyle(BigButtonStyle())
                    .disabled(PairingLink(linkText) == nil || name.trimmingCharacters(in: .whitespaces).isEmpty || working)
                    .accessibilityIdentifier("pairButton")
                Note(text: "Everything stays on your machine. The relay only forwards encrypted requests.")
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: .infinity)
            }
            .padding(.horizontal, 20)
            .padding(.bottom, 8)
        }
    }

    private func confirm(_ fingerprint: String) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Headline("Confirm on your computer.").stateStyle().padding(.top, 28)
            (Text("Check that the terminal running pair-device shows exactly this fingerprint, then type ")
                + Text("yes").font(Face.mono(14)).foregroundColor(Ink.ink) + Text(" there."))
                .font(Face.geist(16)).lineSpacing(4).foregroundStyle(Ink.sub)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, 6)
            CardBox(padding: 22) {
                Text("THIS PHONE’S FINGERPRINT").font(Face.section).tracking(0.84).foregroundStyle(Ink.mut)
                Text(Self.lines(fingerprint))
                    .font(Face.mono(26, .medium, relativeTo: .title))
                    .lineSpacing(6)
                    .foregroundStyle(Ink.ink)
                    .textSelection(.enabled)
                    .accessibilityLabel(fingerprint)
                    .accessibilityIdentifier("fingerprint")
                HStack(spacing: 12) {
                    StatusGlyph(glyph: .work)
                    Text("Waiting for the hub…").font(Face.geist(15)).foregroundStyle(Ink.sub)
                }
                .padding(.top, 14)
                .overlay(alignment: .top) { Hairline() }
            }
            .padding(.top, 28)
            Note(text: "If the two don’t match, cancel. Someone else may have used your pairing link.").padding(.top, 14)
            Spacer()
            Button("Cancel") { pairTask?.cancel() }
                .buttonStyle(BigButtonStyle(kind: .outline))
                .accessibilityIdentifier("cancelPairing")
                .padding(.bottom, 8)
        }
        .padding(.horizontal, 20)
    }

    /// `3c9a-51d7-e08f-22b4-7a16-c4e9` as three lines of two groups.
    static func lines(_ f: String) -> String {
        let groups = f.split(separator: "-").map(String.init)
        guard groups.count > 2 else { return f }
        return stride(from: 0, to: groups.count, by: 2).map { groups[$0..<min($0 + 2, groups.count)].joined(separator: "-") }
            .joined(separator: "\n")
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
