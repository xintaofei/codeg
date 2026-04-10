import type { Metadata, Viewport } from "next"
import "katex/dist/katex.min.css"
import "./globals.css"
import { JetBrains_Mono } from "next/font/google"
import { NextIntlClientProvider } from "next-intl"
import { AppI18nProvider } from "@/components/i18n-provider"
import { getMessagesForLocale } from "@/i18n/messages"
import { resolveRequestLocale } from "@/i18n/resolve-request-locale"
import { ThemeProvider } from "@/components/theme-provider"
import { toIntlLocale } from "@/lib/i18n"

const jetbrainsMono = JetBrains_Mono({
  subsets: ["latin"],
  variable: "--font-sans",
})

export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  viewportFit: "cover",
}

export const metadata: Metadata = {
  title: "codeg",
  description: "AI Coding Agent Conversation Manager",
  icons: {
    icon: [
      { url: "/icon-32x32.png", sizes: "32x32", type: "image/png" },
      { url: "/icon.svg", type: "image/svg+xml" },
    ],
    apple: { url: "/icon-128x128.png", sizes: "128x128", type: "image/png" },
  },
}

export default async function RootLayout({
  children,
}: {
  children: React.ReactNode
}) {
  const appLocale = await resolveRequestLocale()
  const initialLocale = toIntlLocale(appLocale)
  const initialMessages = await getMessagesForLocale(appLocale)

  return (
    <html
      lang={initialLocale}
      className={jetbrainsMono.variable}
      suppressHydrationWarning
    >
      <body>
        {/* Suppress benign ResizeObserver loop warnings (W3C spec §3.3) */}
        <script>{`window.addEventListener("error",function(e){if(e.message&&e.message.indexOf("ResizeObserver")!==-1){e.stopImmediatePropagation();e.preventDefault()}});window.onerror=function(m){if(typeof m==="string"&&m.indexOf("ResizeObserver")!==-1)return true}`}</script>
        {/* Apply appearance settings before paint to prevent flash */}
        <script>{`(function(){try{var s=localStorage.getItem("settings:appearance:v1");if(!s)return;var a=JSON.parse(s);var r=document.documentElement;if(typeof a.uiFontSize==="number"){var u=Math.min(Math.max(a.uiFontSize,12),20);r.style.fontSize=u+"px";var z=u/14;try{r.style.setProperty("zoom",String(z))}catch(e){}r.style.setProperty("--ui-zoom",String(z))}if(typeof a.codeFontSize==="number")r.style.setProperty("--code-font-size",Math.min(Math.max(a.codeFontSize,10),24)+"px")}catch(e){}})()`}</script>
        <NextIntlClientProvider
          locale={initialLocale}
          messages={initialMessages}
        >
          <AppI18nProvider
            initialLocale={initialLocale}
            initialMessages={initialMessages}
          >
            <ThemeProvider
              attribute="class"
              defaultTheme="system"
              enableSystem
              disableTransitionOnChange
            >
              {children}
            </ThemeProvider>
          </AppI18nProvider>
        </NextIntlClientProvider>
      </body>
    </html>
  )
}
