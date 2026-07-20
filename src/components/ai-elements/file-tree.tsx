"use client"

import type {
  ButtonHTMLAttributes,
  CSSProperties,
  HTMLAttributes,
  ReactNode,
} from "react"

import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import { cn } from "@/lib/utils"
import {
  ChevronRightIcon,
  FileIcon,
  FolderIcon,
  FolderOpenIcon,
} from "lucide-react"
import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useState,
} from "react"

interface FileTreeContextType {
  expandedPaths: Set<string>
  togglePath: (path: string) => void
  selectedPath?: string
  onSelect?: (path: string) => void
}

// Row indentation is driven by an explicit `depth` passed from the tree renderer
// (a pure-CSS accumulating variable would have to reference itself, which CSS
// treats as a cycle → invalid → zero padding). When a caller provides `depth`,
// the row stays FULL-WIDTH and indents only its CONTENT via padding-left, so the
// hover / selection / drop highlight spans the whole row at any depth. When
// `depth` is omitted (callers that nest purely via <FileTreeFolder> children),
// the classic margin/border/padding wrapper indentation is used, unchanged.
const FILE_TREE_INDENT_STEP_REM = 1.5
function rowPaddingLeftStyle(
  depth: number | undefined,
  base: CSSProperties | undefined
): CSSProperties | undefined {
  if (depth == null) return base
  return {
    paddingLeft: `calc(${depth} * ${FILE_TREE_INDENT_STEP_REM}rem + 0.5rem)`,
    ...base,
  }
}

// Default noop for context default value
// oxlint-disable-next-line eslint(no-empty-function)
const noop = () => {}

const FileTreeContext = createContext<FileTreeContextType>({
  // oxlint-disable-next-line eslint-plugin-unicorn(no-new-builtin)
  expandedPaths: new Set(),
  togglePath: noop,
})

export type FileTreeProps = Omit<HTMLAttributes<HTMLDivElement>, "onSelect"> & {
  expanded?: Set<string>
  defaultExpanded?: Set<string>
  selectedPath?: string
  onSelect?: (path: string) => void
  onExpandedChange?: (expanded: Set<string>) => void
}

export const FileTree = ({
  expanded: controlledExpanded,
  defaultExpanded = new Set(),
  selectedPath,
  onSelect,
  onExpandedChange,
  className,
  children,
  ...props
}: FileTreeProps) => {
  const [internalExpanded, setInternalExpanded] = useState(defaultExpanded)
  const expandedPaths = controlledExpanded ?? internalExpanded

  const togglePath = useCallback(
    (path: string) => {
      const newExpanded = new Set(expandedPaths)
      if (newExpanded.has(path)) {
        newExpanded.delete(path)
      } else {
        newExpanded.add(path)
      }
      setInternalExpanded(newExpanded)
      onExpandedChange?.(newExpanded)
    },
    [expandedPaths, onExpandedChange]
  )

  const contextValue = useMemo(
    () => ({ expandedPaths, onSelect, selectedPath, togglePath }),
    [expandedPaths, onSelect, selectedPath, togglePath]
  )

  return (
    <FileTreeContext.Provider value={contextValue}>
      <div
        className={cn(
          "rounded-lg border bg-background font-mono text-sm",
          className
        )}
        role="tree"
        {...props}
      >
        <div className="w-max min-w-full">{children}</div>
      </div>
    </FileTreeContext.Provider>
  )
}

interface FileTreeFolderContextType {
  path: string
  name: string
  isExpanded: boolean
}

const FileTreeFolderContext = createContext<FileTreeFolderContextType>({
  isExpanded: false,
  name: "",
  path: "",
})

export type FileTreeFolderProps = HTMLAttributes<HTMLDivElement> & {
  path: string
  name: string
  nameClassName?: string
  iconClassName?: string
  suffix?: ReactNode
  suffixClassName?: string
  /**
   * Props applied to the folder's header row (the trigger button) — e.g.
   * `draggable` and drag/drop handlers for file-tree DnD. Placed on the header
   * (not the outer wrapper, which also contains the child rows) so a drop
   * targets THIS folder rather than its whole subtree. `onClick`/`type` are
   * owned by the folder and are not overridable here.
   */
  rowProps?: ButtonHTMLAttributes<HTMLButtonElement>
  /** Render a drop-target highlight on the header row (a valid DnD drop is
   *  hovering this folder). */
  dropActive?: boolean
  /**
   * Marks this folder's header row as a directory drop zone for file-tree DnD,
   * tagging it with `data-tree-drop-dir` set to this value (the destination
   * path relative to the workspace root; `""` for the root row). The desktop
   * drop path hit-tests the drop coordinates against these markers because
   * Tauri's webview swallows the HTML5 `drop` event. Omit on non-DnD trees.
   */
  dropTargetDir?: string
  /**
   * Nesting depth (0 = top level). When provided, the row is rendered
   * full-width and its content is indented via padding-left instead of an
   * inset children wrapper, so the row highlight spans the whole tree at any
   * depth. When omitted, the classic wrapper indentation is used unchanged.
   */
  depth?: number
}

export const FileTreeFolder = ({
  path,
  name,
  nameClassName,
  iconClassName,
  suffix,
  suffixClassName,
  rowProps,
  dropActive,
  dropTargetDir,
  depth,
  className,
  children,
  ...props
}: FileTreeFolderProps) => {
  const {
    className: rowClassName,
    style: rowStyle,
    ...rowRest
  } = rowProps ?? {}
  const { expandedPaths, togglePath, selectedPath, onSelect } =
    useContext(FileTreeContext)
  const isExpanded = expandedPaths.has(path)
  const isSelected = selectedPath === path

  const handleOpenChange = useCallback(() => {
    togglePath(path)
  }, [togglePath, path])

  const handleSelect = useCallback(() => {
    onSelect?.(path)
  }, [onSelect, path])

  const folderContextValue = useMemo(
    () => ({ isExpanded, name, path }),
    [isExpanded, name, path]
  )

  return (
    <FileTreeFolderContext.Provider value={folderContextValue}>
      <Collapsible onOpenChange={handleOpenChange} open={isExpanded}>
        <div
          className={cn("", className)}
          aria-selected={isSelected}
          role="treeitem"
          tabIndex={0}
          {...props}
        >
          <CollapsibleTrigger asChild>
            <button
              className={cn(
                "flex w-max min-w-full items-center gap-1 rounded py-1 text-left transition-colors",
                depth == null ? "px-2" : "pr-2",
                // A selected row — or a directory being hovered as a drop target
                // — gets the same static tint with NO hover change; only idle
                // rows show the hover affordance.
                dropActive || isSelected
                  ? "bg-muted-foreground/20"
                  : "hover:bg-muted/50",
                rowClassName
              )}
              style={rowPaddingLeftStyle(depth, rowStyle)}
              onClick={handleSelect}
              type="button"
              data-tree-drop-dir={dropTargetDir}
              {...rowRest}
            >
              <ChevronRightIcon
                className={cn(
                  "size-4 shrink-0 text-muted-foreground transition-transform",
                  isExpanded && "rotate-90"
                )}
              />
              <FileTreeIcon>
                {isExpanded ? (
                  <FolderOpenIcon
                    className={cn("size-4 text-blue-500", iconClassName)}
                  />
                ) : (
                  <FolderIcon
                    className={cn("size-4 text-blue-500", iconClassName)}
                  />
                )}
              </FileTreeIcon>
              <FileTreeName className={nameClassName}>{name}</FileTreeName>
              {suffix ? (
                <span
                  className={cn(
                    "ml-1 shrink-0 whitespace-nowrap text-muted-foreground/60",
                    suffixClassName
                  )}
                >
                  {suffix}
                </span>
              ) : null}
            </button>
          </CollapsibleTrigger>
          <CollapsibleContent>
            {/* With explicit `depth`, descendants indent themselves via
                padding, so this wrapper adds NO left inset (keeping their
                highlights full-width). Without it, fall back to the classic
                margin/border/padding inset. */}
            <div className={depth == null ? "ml-4 border-l pl-2" : undefined}>
              {children}
            </div>
          </CollapsibleContent>
        </div>
      </Collapsible>
    </FileTreeFolderContext.Provider>
  )
}

interface FileTreeFileContextType {
  path: string
  name: string
}

const FileTreeFileContext = createContext<FileTreeFileContextType>({
  name: "",
  path: "",
})

export type FileTreeFileProps = HTMLAttributes<HTMLDivElement> & {
  path: string
  name: string
  icon?: ReactNode
  /** Nesting depth (0 = top level). See {@link FileTreeFolderProps.depth}: when
   *  provided the row is full-width and indents its content via padding. */
  depth?: number
}

export const FileTreeFile = ({
  path,
  name,
  icon,
  depth,
  className,
  style,
  children,
  ...props
}: FileTreeFileProps) => {
  const { selectedPath, onSelect } = useContext(FileTreeContext)
  const isSelected = selectedPath === path

  const handleClick = useCallback(() => {
    onSelect?.(path)
  }, [onSelect, path])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" || e.key === " ") {
        onSelect?.(path)
      }
    },
    [onSelect, path]
  )

  const fileContextValue = useMemo(() => ({ name, path }), [name, path])

  return (
    <FileTreeFileContext.Provider value={fileContextValue}>
      <div
        className={cn(
          "flex w-max min-w-full cursor-pointer items-center gap-1 rounded py-1 transition-colors",
          depth == null ? "px-2" : "pr-2",
          // Selected rows keep a static tint (no hover change); idle rows show
          // the hover affordance.
          isSelected ? "bg-muted-foreground/20" : "hover:bg-muted/50",
          className
        )}
        style={rowPaddingLeftStyle(depth, style)}
        onClick={handleClick}
        onKeyDown={handleKeyDown}
        aria-selected={isSelected}
        role="treeitem"
        tabIndex={0}
        {...props}
      >
        {children ?? (
          <>
            {/* Spacer for alignment */}
            <span className="size-4" />
            <FileTreeIcon>
              {icon ?? <FileIcon className="size-4 text-muted-foreground" />}
            </FileTreeIcon>
            <FileTreeName>{name}</FileTreeName>
          </>
        )}
      </div>
    </FileTreeFileContext.Provider>
  )
}

export type FileTreeIconProps = HTMLAttributes<HTMLSpanElement>

export const FileTreeIcon = ({
  className,
  children,
  ...props
}: FileTreeIconProps) => (
  <span className={cn("shrink-0", className)} {...props}>
    {children}
  </span>
)

export type FileTreeNameProps = HTMLAttributes<HTMLSpanElement>

export const FileTreeName = ({
  className,
  children,
  ...props
}: FileTreeNameProps) => (
  <span className={cn("shrink-0 whitespace-nowrap", className)} {...props}>
    {children}
  </span>
)

export type FileTreeActionsProps = HTMLAttributes<HTMLDivElement>

const stopPropagation = (e: React.SyntheticEvent) => e.stopPropagation()

export const FileTreeActions = ({
  className,
  children,
  ...props
}: FileTreeActionsProps) => (
  // biome-ignore lint/a11y/noNoninteractiveElementInteractions: stopPropagation required for nested interactions
  // biome-ignore lint/a11y/useSemanticElements: fieldset doesn't fit this UI pattern
  <div
    className={cn("ml-auto flex items-center gap-1", className)}
    onClick={stopPropagation}
    onKeyDown={stopPropagation}
    role="group"
    {...props}
  >
    {children}
  </div>
)
