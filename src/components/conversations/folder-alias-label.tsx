import { cn } from "@/lib/utils"

/**
 * Renders a folder label. When the folder has a user-set alias it shows
 * `alias [ name ]`, with the bracketed original name in a color a bit *deeper*
 * than the surrounding folder-name text so the chosen alias and the real folder
 * name read as clearly separate. With no alias (or a blank one) it renders the
 * bare `name`.
 *
 * "Deeper" is surface-relative (the base name color differs between the sidebar
 * and the conversation header), so each call site passes its own
 * `bracketClassName`; the default is a neutral fallback.
 *
 * The plain-string equivalent (for `title` tooltips and other string-only
 * contexts) is `formatFolderLabelWithAlias` in `@/lib/folder-display` — keep the
 * two in sync (same `alias [ name ]` spacing).
 */
export function FolderAliasLabel({
  name,
  alias,
  bracketClassName,
}: {
  name: string
  alias: string | null
  /** Color for the `[ name ]` segment — pass a shade a bit deeper than the
   *  caller's base folder-name color. */
  bracketClassName?: string
}) {
  const trimmed = alias?.trim()
  if (!trimmed) return <>{name}</>
  return (
    <>
      {trimmed}{" "}
      <span className={cn("text-foreground/90", bracketClassName)}>
        [ {name} ]
      </span>
    </>
  )
}
