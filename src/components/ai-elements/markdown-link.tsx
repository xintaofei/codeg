"use client"

import type { ComponentProps, MouseEvent } from "react"
import { useCallback, useState } from "react"
import { FileText, Globe, Mail, Phone, type LucideIcon } from "lucide-react"
import type { Components, LinkSafetyModalProps } from "streamdown"

import { classifyResourceKind, type ResourceKind } from "@/lib/resource-kind"
import { cn } from "@/lib/utils"
import { useStreamdownLinkSafety } from "./link-safety"

const RESOURCE_KIND_ICON: Record<ResourceKind, LucideIcon> = {
  file: FileText,
  web: Globe,
  email: Mail,
  phone: Phone,
}

// Streamdown swaps the href of a not-yet-closed markdown link with this
// sentinel while the message is still streaming.
const INCOMPLETE_LINK = "streamdown:incomplete-link"

type MarkdownLinkProps = ComponentProps<"a"> & {
  // react-markdown passes the originating hast node; it must not reach the DOM.
  node?: unknown
}

/**
 * Anchor override for markdown rendered by `<Streamdown>` (chat messages and
 * reasoning blocks). It mirrors Streamdown's built-in link element — a
 * `<button>` whose clicks are routed through the shared link-safety config
 * (file → workspace panel, http(s) → browser, mailto/tel → OS handler) plus
 * its modal hook — and additionally prepends a small type icon so users can
 * tell at a glance whether an address is a file, a web link, an email, or a
 * phone number.
 *
 * Overriding `components.a` is the right layer for this: the icon is a React
 * node, so it must be injected after rehype-sanitize (which would strip an
 * element/attribute added upstream in a remark/rehype plugin).
 */
export function MarkdownLink({
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  node,
  href,
  children,
  className,
  ...rest
}: MarkdownLinkProps) {
  const linkSafety = useStreamdownLinkSafety()
  const [modalOpen, setModalOpen] = useState(false)

  const isIncomplete = href === INCOMPLETE_LINK

  const handleClick = useCallback(
    async (event: MouseEvent<HTMLButtonElement>) => {
      if (!href || isIncomplete) return
      event.preventDefault()
      if (linkSafety.onLinkCheck && (await linkSafety.onLinkCheck(href))) {
        window.open(href, "_blank", "noreferrer")
        return
      }
      setModalOpen(true)
    },
    [href, isIncomplete, linkSafety]
  )

  // No usable href: render an inert anchor, matching Streamdown's fallback.
  if (!href) {
    return (
      <a
        className={cn(
          "wrap-anywhere font-medium text-primary underline",
          className
        )}
        {...rest}
      >
        {children}
      </a>
    )
  }

  const kind = isIncomplete ? null : classifyResourceKind(href)
  const Icon = kind ? RESOURCE_KIND_ICON[kind] : null

  const modalProps: LinkSafetyModalProps = {
    url: href,
    isOpen: modalOpen,
    onClose: () => setModalOpen(false),
    onConfirm: () => window.open(href, "_blank", "noreferrer"),
  }

  return (
    <>
      <button
        type="button"
        data-incomplete={isIncomplete}
        data-streamdown="link"
        data-resource-kind={kind ?? undefined}
        title={isIncomplete ? undefined : href}
        onClick={handleClick}
        className={cn(
          "wrap-anywhere appearance-none text-left font-medium text-primary underline",
          className
        )}
      >
        {Icon ? (
          <Icon
            aria-hidden="true"
            className="mr-0.5 inline size-[1em] align-[-0.15em] opacity-80"
          />
        ) : null}
        {children}
      </button>
      {linkSafety.renderModal ? linkSafety.renderModal(modalProps) : null}
    </>
  )
}

// react-markdown's `Components` map carries a string index signature that forces
// every element override to accept `Record<string, unknown>` props, which is
// incompatible with MarkdownLink's precise anchor props. The cast bridges that
// gap — MarkdownLink receives exactly the props react-markdown passes for `a`.
export const markdownLinkComponents: Components = {
  a: MarkdownLink as Components["a"],
}
