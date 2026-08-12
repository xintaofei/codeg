import { getTransport } from "./transport"
import { isDesktop } from "./transport"

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
  if (isDesktop()) {
    await getTransport().call("send_notification", { title, body, target })
  } else {
    if (!document.hidden && document.hasFocus()) return
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
