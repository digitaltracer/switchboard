import SwiftUI
import SwitchboardKit

/// "Ship it": buttons that ask a finished agent to run one of the owner's own skills. Switchboard
/// never runs git or gh; a push still comes to the phone as an approval.
struct ShipCard: View {
    @Environment(AppModel.self) private var model
    let target: Schemas.TargetView
    let project: String?
    let branch: String?
    let status: String
    /// The agent's latest reply, and when it was written.
    let reply: String?
    let replyAt: Date?
    @State private var skills: [Schemas.Skill] = []
    @State private var confirming: ShipButton?

    private var key: String { AppModel.saidKey(for: target) }
    private var finished: Bool { ["idle", "done"].contains(status) }

    private var buttons: [ShipButton] {
        let set = model.shipButtons.map { ShipButtons.effective($0, project: project) } ?? []
        return set.isEmpty ? ShipButtons.suggested(from: skills.map(\.name)) : set
    }

    /// How the last ship skill went, judged from the reply written after it was sent.
    private var outcome: ShipOutcome? {
        guard let sent = model.lastShip[key] else { return nil }
        let after = replyAt.map { $0 >= sent.at } ?? false
        return ShipOutcome.of(sent: sent.button, turnEnded: finished && after, reply: after ? reply : nil)
    }

    private var pullRequest: PullRequest? {
        if case .opened(let pr) = outcome { return pr }
        return reply.flatMap(PullRequest.find(in:))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if let sent = model.lastShip[key], outcome == .running {
                Note(text: "\(target.label) is running \(sent.button.skill)…")
            }
            if outcome == .noPullRequest {
                Note(text: "No pull request was opened. See the agent's reply.", color: Ink.warn)
                    .accessibilityIdentifier("noPullRequest")
            }
            if let pr = pullRequest {
                PRCard(pr: pr)
                let next = buttons.filter(\.afterPR)
                if !next.isEmpty, finished {
                    SectionHead("Next, with your skills").padding(.top, -14).padding(.bottom, -8)
                    grid(next, primary: next.first)
                }
            } else if finished, outcome != .running {
                shipIt
            }
        }
        .padding(.top, 14)
        .task(id: target.ref.sessionId ?? target.agent) { await loadSkills() }
        .sheet(item: $confirming) { b in
            ShipConfirmSheet(button: b.invocation(for: skills.map(\.name)), target: target, branch: branch) { confirming = nil }
        }
    }

    private var shipIt: some View {
        let main = buttons.filter { !$0.afterPR }
        return VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline) {
                Text("Ship it").font(Face.geist(17, .semibold)).foregroundStyle(Ink.ink)
                Spacer()
                Text("Your skills").font(Face.meta).foregroundStyle(Ink.mut)
            }
            if !main.isEmpty { grid(main, primary: main.first) }
            if !skills.isEmpty {
                Menu {
                    ForEach(skills, id: \.name) { s in
                        Button(s.name) { confirming = ShipButton(skill: s.name, label: ShipButtons.label(for: s.name)) }
                    }
                } label: {
                    Text("Run another skill…").font(Face.geist(15, .medium)).foregroundStyle(Ink.ac)
                }
                .accessibilityIdentifier("runAnotherSkill")
            }
            Hairline()
            Text("Switchboard asks the agent to run your skill. A push, or anything else that needs your OK, still comes to you as an approval.")
                .font(Face.meta).foregroundStyle(Ink.sub).fixedSize(horizontal: false, vertical: true)
            if buttons.contains(where: \.afterPR) {
                Label("\(buttons.first(where: \.afterPR)!.label) appears once a pull request is open.", systemImage: "info.circle")
                    .font(Face.meta).foregroundStyle(Ink.mut)
            }
        }
        .padding(16)
        .background(Ink.sf, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay { RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(Ink.line) }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("shipIt")
    }

    /// The main button full width and black, the rest two to a row.
    private func grid(_ list: [ShipButton], primary: ShipButton?) -> some View {
        let rest = list.filter { $0 != primary }
        return VStack(spacing: 10) {
            if let primary {
                Button { confirming = primary } label: {
                    HStack(spacing: 12) {
                        Image(systemName: "arrow.triangle.pull").font(.system(size: 17, weight: .medium))
                        VStack(alignment: .leading, spacing: 1) {
                            Text(primary.label).font(Face.geist(16, .semibold))
                            Text(primary.skill).font(Face.mono(12)).opacity(0.7)
                        }
                        Spacer()
                        Image(systemName: "chevron.right").font(.system(size: 13, weight: .semibold))
                    }
                    .foregroundStyle(Ink.inv)
                    .padding(.horizontal, 16).frame(minHeight: 56)
                    .background(Ink.ink, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("ship-\(primary.skill)")
            }
            LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible())], spacing: 10) {
                ForEach(rest) { b in
                    Button { confirming = b } label: {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(b.label).font(Face.geist(15, .semibold)).foregroundStyle(Ink.ink).lineLimit(1)
                            Text(b.skill).font(Face.mono(12)).foregroundStyle(Ink.mut).lineLimit(1)
                        }
                        .frame(maxWidth: .infinity, minHeight: 56, alignment: .leading)
                        .padding(.horizontal, 14)
                        .background(Ink.sf, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                        .overlay { RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Ink.line, lineWidth: 1.5) }
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("ship-\(b.skill)")
                }
            }
        }
    }

    private func loadSkills() async {
        guard let hub = model.hub else { return }
        if let id = target.sessionId {
            skills = (try? await hub.skills(id)) ?? []
        } else if let agent = target.agent {
            skills = (try? await hub.agentSkills(machine: target.machineId, agent: agent)) ?? []
        }
    }
}

/// A pull request found in the agent's reply, with a button that opens it.
struct PRCard: View {
    let pr: PullRequest
    @Environment(\.openURL) private var openURL

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                Image(systemName: "arrow.triangle.pull").foregroundStyle(Ink.ok)
                    .frame(width: 36, height: 36).background(Ink.ok.opacity(0.12), in: RoundedRectangle(cornerRadius: 10))
                VStack(alignment: .leading, spacing: 2) {
                    Text("Pull request opened").font(Face.geist(16, .semibold)).foregroundStyle(Ink.ink)
                    Text("\(pr.repo) · #\(pr.number)").font(Face.meta).foregroundStyle(Ink.sub)
                }
            }
            if let t = pr.title { Text(t).font(Face.geist(15, .medium)).foregroundStyle(Ink.ink) }
            Text(pr.url.absoluteString.replacingOccurrences(of: "https://", with: ""))
                .font(Face.mono(12.5)).foregroundStyle(Ink.sub).lineLimit(1).truncationMode(.middle)
            Button { openURL(pr.url) } label: { Label("Open pull request", systemImage: "arrow.up.right") }
                .buttonStyle(BigButtonStyle(kind: .outline))
                .accessibilityIdentifier("openPullRequest")
            Text("Found in the agent's reply. Switchboard doesn't call GitHub.").font(Face.meta).foregroundStyle(Ink.mut)
        }
        .padding(16)
        .background(Ink.sf, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay { RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(Ink.line) }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("pullRequest")
    }
}

/// Confirms a ship skill, with optional instructions after it, then sends it as a message.
struct ShipConfirmSheet: View {
    @Environment(AppModel.self) private var model
    let button: ShipButton
    let target: Schemas.TargetView
    let branch: String?
    let done: () -> Void
    @State private var instructions = ""
    @State private var sending = false
    @State private var note: String?
    @State private var requestId = UUID().uuidString

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Run \(button.skill) on \(target.label)?").font(Face.geist(24, .semibold)).foregroundStyle(Ink.ink)
                .fixedSize(horizontal: false, vertical: true)
            (Text("The agent runs your ") + Text(button.skill).font(Face.mono(14)) + Text(" skill")
                + (branch.map { Text(" on branch ") + Text($0).font(Face.mono(14)) } ?? Text("")) + Text("."))
                .font(Face.geist(15)).foregroundStyle(Ink.sub)
                .fixedSize(horizontal: false, vertical: true)
            SectionHead("Instructions for the skill (optional)").padding(.bottom, -6)
            TextField("Draft, base main", text: $instructions)
                .font(Face.geist(17))
                .padding(14)
                .background(Ink.sf, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                .overlay { RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Ink.line) }
                .accessibilityIdentifier("shipInstructions")
            SectionHead("Message the agent gets").padding(.bottom, -6)
            Text(button.message(instructions))
                .font(Face.mono(13.5)).foregroundStyle(Ink.codeText)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(14)
                .background(Ink.code, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            Label {
                Text("It runs with the agent's own permissions. ") + Text("git push").font(Face.geist(14, .semibold))
                    + Text(" and other commands that need your OK go to Needs you, signed with Face ID.")
            } icon: {
                Image(systemName: "checkmark.shield").foregroundStyle(Ink.warn)
            }
            .font(Face.geist(14)).foregroundStyle(Ink.sub)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(14)
            .background(Ink.sf, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay { RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Ink.line) }
            if let note { Note(text: note, color: Ink.bad) }
            Spacer(minLength: 0)
            HStack(spacing: 12) {
                Button("Cancel", action: done).buttonStyle(BigButtonStyle(kind: .outline)).frame(maxWidth: 140)
                Button(sending ? "Sending…" : "Run \(button.skill)") { Task { await send() } }
                    .buttonStyle(BigButtonStyle())
                    .disabled(sending)
                    .accessibilityIdentifier("shipRun")
            }
        }
        .padding(.horizontal, 20)
        .padding(.top, 30)
        .padding(.bottom, 20)
        .background(Ink.bg)
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
    }

    private func send() async {
        guard let hub = model.hub else { return }
        sending = true
        defer { sending = false }
        let text = button.message(instructions)
        do {
            let r = try await hub.message(text, requestId: requestId, current: nil, target: target)
            if r.outcome == "sent" {
                model.lastShip[AppModel.saidKey(for: target)] = .init(button: button, at: Date())
                model.record(sent: text, to: r.current ?? target, started: false)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                done()
            } else {
                note = r.message
            }
        } catch {
            note = error.localizedDescription
        }
    }
}
