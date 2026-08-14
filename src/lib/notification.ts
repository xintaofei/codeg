import { getTransport } from "./transport"
import { isDesktop } from "./transport"
import { getNotificationSoundPrefs } from "./notification-sound-prefs"

export interface NotificationTarget {
  folderId: number
  conversationId: number
  agent: string
}

export async function sendSystemNotification(
  title: string,
  body: string,
  target?: NotificationTarget | null
): Promise<void> {
  const prefs = getNotificationSoundPrefs()
  if (!prefs.systemNotificationsEnabled) return
  if (
    prefs.systemNotificationsOnlyWhenUnfocused &&
    !document.hidden &&
    document.hasFocus()
  ) {
    return
  }

  if (isDesktop()) {
    await getTransport().call("send_notification", { title, body, target })
  } else {
    // Web fallback: Browser Notification API
    if (Notification.permission === "granted") {
      new Notification(title, { body })
    } else if (Notification.permission !== "denied") {
      const permission = await Notification.requestPermission()
      if (permission === "granted") {
        new Notification(title, { body })
      }
    }
  }
}
