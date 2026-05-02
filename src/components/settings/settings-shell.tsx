"use client"

import {
  useCallback,
  useEffect,
  useState,
  type ComponentType,
  type ReactNode,
} from "react"
import {
  Bot,
  BookOpenText,
  GitBranch,
  Globe,
  Keyboard,
  Menu,
  MessageSquareText,
  SendHorizontal,
  Palette,
  PlugZap,
  Server,
  Settings,
  Sparkles,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { usePathname } from "next/navigation"
import { useRouter } from "next/navigation"
import { Button } from "@/components/ui/button"
import { AppToaster } from "@/components/ui/app-toaster"
import { cn } from "@/lib/utils"
import { detectEnvironment } from "@/lib/transport/detect"
import { AppTitleBar } from "@/components/layout/app-title-bar"
import { useIsMobile } from "@/hooks/use-mobile"
import { Sheet, SheetContent, SheetTitle } from "@/components/ui/sheet"

interface SettingsNavItem {
  href: string
  labelKey:
    | "appearance"
    | "agents"
    | "model_providers"
    | "mcp"
    | "skills"
    | "experts"
    | "quick_messages"
    | "shortcuts"
    | "version_control"
    | "chat_channels"
    | "system"
    | "web_service"
  icon: ComponentType<{ className?: string }>
}

const SETTINGS_NAV_ITEMS: SettingsNavItem[] = [
  {
    href: "/settings/appearance",
    labelKey: "appearance",
    icon: Palette,
  },
  {
    href: "/settings/mcp",
    labelKey: "mcp",
    icon: PlugZap,
  },
  {
    href: "/settings/skills",
    labelKey: "skills",
    icon: BookOpenText,
  },
  {
    href: "/settings/experts",
    labelKey: "experts",
    icon: Sparkles,
  },
  {
    href: "/settings/agents",
    labelKey: "agents",
    icon: Bot,
  },
  {
    href: "/settings/quick-messages",
    labelKey: "quick_messages",
    icon: MessageSquareText,
  },
  {
    href: "/settings/model-providers",
    labelKey: "model_providers",
    icon: Server,
  },
  {
    href: "/settings/shortcuts",
    labelKey: "shortcuts",
    icon: Keyboard,
  },
  {
    href: "/settings/version-control",
    labelKey: "version_control",
    icon: GitBranch,
  },
  {
    href: "/settings/chat-channels",
    labelKey: "chat_channels",
    icon: SendHorizontal,
  },
  {
    href: "/settings/web-service",
    labelKey: "web_service",
    icon: Globe,
  },
  {
    href: "/settings/system",
    labelKey: "system",
    icon: Settings,
  },
]

interface SettingsShellProps {
  children: ReactNode
}

function normalizePath(path: string): string {
  const noSuffix = path.replace(/\/index\.html$/, "").replace(/\.html$/, "")
  const noTrailingSlash = noSuffix.replace(/\/+$/, "")
  return noTrailingSlash || "/"
}

function isWindowsRuntime(): boolean {
  if (typeof navigator === "undefined") return false
  const platform = navigator.platform.toLowerCase()
  const userAgent = navigator.userAgent.toLowerCase()
  return platform.includes("win") || userAgent.includes("windows")
}

export function SettingsShell({ children }: SettingsShellProps) {
  const t = useTranslations("SettingsShell")
  const pathname = usePathname()
  const router = useRouter()
  const normalizedPathname = normalizePath(pathname)
  const isMobile = useIsMobile()
  const [navOpen, setNavOpen] = useState(false)

  useEffect(() => {
    document.title = `${t("title")} - codeg`
  }, [t])

  const navigateTo = useCallback(
    (href: string) => {
      if (typeof window === "undefined") return

      const target = normalizePath(href)
      const current = normalizePath(window.location.pathname)
      if (current === target) {
        setNavOpen(false)
        return
      }

      if (isWindowsRuntime()) {
        window.location.assign(target)
        return
      }

      router.push(target)
      setNavOpen(false)
    },
    [router, setNavOpen]
  )

  const filteredNavItems = SETTINGS_NAV_ITEMS.filter(
    (item) =>
      !(item.labelKey === "web_service" && detectEnvironment() === "web")
  )

  const navContent = (
    <>
      <div className="px-1 pb-2 text-[11px] font-medium text-muted-foreground">
        {t("preferences")}
      </div>
      <nav className="space-y-1">
        {filteredNavItems.map((item) => {
          const Icon = item.icon
          const translationKey = `nav.${item.labelKey}` as const
          const active =
            normalizedPathname === item.href ||
            normalizedPathname.startsWith(`${item.href}/`)
          return (
            <Button
              key={item.href}
              variant={active ? "secondary" : "ghost"}
              size="sm"
              className={cn("w-full justify-start")}
              type="button"
              onClick={() => navigateTo(item.href)}
              aria-current={active ? "page" : undefined}
            >
              <span className="inline-flex items-center gap-1">
                <Icon className="h-3.5 w-3.5" />
                {t(translationKey)}
              </span>
            </Button>
          )
        })}
      </nav>
    </>
  )

  return (
    <div className="h-screen flex flex-col overflow-hidden bg-background text-foreground">
      <AppTitleBar
        left={
          isMobile ? (
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8"
              onClick={() => setNavOpen(true)}
            >
              <Menu className="h-4 w-4" />
            </Button>
          ) : undefined
        }
        center={
          <div className="text-sm font-bold tracking-tight">{t("title")}</div>
        }
      />

      <div className="flex-1 min-h-0 flex">
        {/* Desktop sidebar */}
        {!isMobile && (
          <aside className="w-56 shrink-0 border-r p-3">{navContent}</aside>
        )}

        {/* Mobile navigation Sheet */}
        {isMobile && (
          <Sheet open={navOpen} onOpenChange={setNavOpen}>
            <SheetContent
              side="left"
              showCloseButton={false}
              className="w-[260px] p-3"
            >
              <SheetTitle className="sr-only">{t("title")}</SheetTitle>
              {navContent}
            </SheetContent>
          </Sheet>
        )}

        <section className="flex-1 min-w-0 min-h-0 overflow-hidden">
          {children}
        </section>
      </div>
      <AppToaster position="bottom-right" closeButton duration={4000} />
    </div>
  )
}
