import SwiftUI
import SwitchboardKit
import UserNotifications

@main
struct SwitchboardApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @Environment(\.scenePhase) private var scenePhase
    @AppStorage(Appearance.key) private var appearance = Appearance.system

    var body: some Scene {
        WindowGroup {
            RootView()
                .preferredColorScheme(appearance.scheme)
                .environment(delegate.model)
                .environment(delegate.voice)
                .onChange(of: delegate.model.vocabulary, initial: true) { _, v in delegate.voice.vocabulary = v }
        }
        .onChange(of: scenePhase) { old, new in
            if new == .active, old != .active { delegate.model.becameActive() }
        }
    }
}

/// Push registration and notification taps. A notification carries ids only; the app always
/// fetches the current state from the hub before showing or acting on anything.
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    @MainActor let model = AppModel()
    @MainActor let voice = Voice()

    func application(_ application: UIApplication, didFinishLaunchingWithOptions _: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        Task { @MainActor in await model.receivedPushToken(deviceToken) }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        // Expected without an APNs entitlement, e.g. an unsigned simulator build.
        print("push registration failed: \(error.localizedDescription)")
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        done([.banner, .sound, .list])
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler done: @escaping () -> Void
    ) {
        let sessionId = response.notification.request.content.userInfo["session_id"] as? String
        Task { @MainActor in self.model.openSessionId = sessionId }
        done()
    }
}
