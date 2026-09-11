import SwiftUI

@MainActor
public struct ConnectionManagerView: View {
    private let manager: ConnectionManager
    @State private var connectDraft: ConnectDraft?

    public init(manager: ConnectionManager) {
        self.manager = manager
    }

    public var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Connections")
                    .font(.title2.bold())
                Spacer()
                Button {
                    connectDraft = ConnectDraft(label: "", host: "", port: "", user: "")
                } label: {
                    Label("Add Connection", systemImage: "plus")
                }
            }
            .padding()

            Divider()

            Group {
                if manager.entries.isEmpty {
                    ContentUnavailableView(
                        "No Saved Connections",
                        systemImage: "network",
                        description: Text("Add an SSH host to start an SRUI session.")
                    )
                } else {
                    List(manager.entries) { entry in
                        SavedConnectionRow(
                            entry: entry,
                            status: manager.status(for: entry.id),
                            onConnect: { manager.connect(id: entry.id) },
                            onOpen: { manager.open(id: entry.id) },
                            onRemove: { manager.remove(id: entry.id) }
                        )
                    }
                }
            }
        }
        .frame(minWidth: 620, minHeight: 360)
        .sheet(item: $connectDraft) { draft in
            ConnectSheet(initialDraft: draft) { submittedDraft in
                _ = manager.connect(submittedDraft)
            }
        }
        .alert(
            manager.alert?.title ?? "Connection Error",
            isPresented: Binding(
                get: { manager.alert != nil },
                set: { presented in
                    if !presented {
                        manager.dismissAlert()
                    }
                }
            )
        ) {
            Button("OK") {
                manager.dismissAlert()
            }
        } message: {
            Text(manager.alert?.message ?? "")
        }
    }
}

@MainActor
private struct SavedConnectionRow: View {
    let entry: SavedConnection
    let status: ConnectionStatus
    let onConnect: () -> Void
    let onOpen: () -> Void
    let onRemove: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: status.symbolName)
                .foregroundStyle(status.tint)
                .font(.title3)
                .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 3) {
                Text(entry.label)
                    .font(.headline)
                Text(endpoint)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                if let sessionID = entry.sessionID {
                    Text("Session \(sessionID) · revision \(entry.lastKnownRevision)")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                }
            }

            Spacer(minLength: 16)

            Text(status.displayText)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.trailing)
                .accessibilityLabel("\(entry.label) status: \(status.displayText)")

            Button(status.primaryAction.title, action: performPrimaryAction)
                .disabled(status.primaryAction.isEnabled == false)
                .accessibilityLabel("\(status.primaryAction.title) \(entry.label)")
            Button("Remove", role: .destructive, action: onRemove)
                .accessibilityLabel("Remove \(entry.label)")
        }
        .padding(.vertical, 5)
    }

    private func performPrimaryAction() {
        switch status.primaryAction {
        case .connect:
            onConnect()
        case .open:
            onOpen()
        case .unavailable:
            break
        }
    }

    private var endpoint: String {
        let portSuffix = entry.port.map { ":\($0)" } ?? ""
        return "\(entry.user)@\(entry.host)\(portSuffix)"
    }
}

@MainActor
private struct ConnectSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var draft: ConnectDraft

    let onConnect: (ConnectDraft) -> Void

    init(initialDraft: ConnectDraft, onConnect: @escaping (ConnectDraft) -> Void) {
        _draft = State(initialValue: initialDraft)
        self.onConnect = onConnect
    }

    var body: some View {
        NavigationStack {
            Form {
                TextField("Name", text: $draft.label)
                TextField("Host", text: $draft.host)
                TextField("Port", text: $draft.port)
                TextField("Remote User", text: $draft.user)

                Section {
                    Text("Authentication and host verification use your existing SSH agent and known_hosts configuration. Credentials are never stored by SRUI.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .formStyle(.grouped)
            .navigationTitle("New Connection")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        dismiss()
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Connect") {
                        onConnect(draft)
                        dismiss()
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!isValid)
                }
            }
        }
        .frame(width: 440, height: 300)
    }

    private var isValid: Bool {
        draft.validationMessage == nil
    }
}

private extension ConnectionStatus {
    var symbolName: String {
        switch self {
        case .unknown:
            "questionmark.circle"
        case .connecting:
            "arrow.triangle.2.circlepath"
        case .resynchronizing:
            "arrow.clockwise.circle"
        case .connected:
            "checkmark.circle.fill"
        case .disconnected:
            "bolt.horizontal.circle"
        }
    }

    var tint: Color {
        switch self {
        case .unknown, .disconnected:
            .gray
        case .connecting, .resynchronizing:
            .orange
        case .connected:
            .green
        }
    }
}
