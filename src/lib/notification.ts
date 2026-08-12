import { getTransport } from "./transport"
import { isDesktop } from "./transport"

export async function sendSystemNotification(
  title: string,
  body: string
): Promise<void> {
  if (isDesktop()) {
    await getTransport().call("send_notification", { title, body })
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
