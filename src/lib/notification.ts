import { getTransport } from "./transport"
import { isDesktop } from "./transport"

export async function sendSystemNotification(
  title: string,
  body: string
): Promise<void> {
  if (!document.hidden && document.hasFocus()) return
  if (isDesktop()) {
    await getTransport().call("send_notification", { title, body })
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
